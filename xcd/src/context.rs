// Copyright (c) 2023 Yan Ka, Chiu.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions, and the following disclaimer,
//    without modification, immediately at the beginning of the file.
// 2. The name of the author may not be used to endorse or promote products
//    derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
// ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE FOR
// ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
// OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
// HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
// LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
// OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
// SUCH DAMAGE.

use crate::auth::Credential;
use crate::config::XcConfig;
use crate::database::Database;
use crate::devfs_store::DevfsRulesetStore;
use crate::image::pull::PullImageError;
use crate::image::ImageManager;
use crate::instantiate::{AppliedInstantiateRequest, InstantiateBlueprint};
use crate::ipc::InstantiateRequest;
use crate::port::PortForwardTable;
use crate::registry::JsonRegistryProvider;
use crate::resources::volume::{Volume, VolumeDriverKind};
use crate::resources::Resources;
use crate::site::Site;
use crate::util::TwoWayMap;

use anyhow::Context;
use freebsd::fs::zfs::ZfsHandle;
use freebsd::libc::EIO;
use freebsd::net::pf;
use oci_util::digest::OciDigest;
use oci_util::image_reference::{ImageReference, ImageTag};
use oci_util::layer::ChainId;
use std::collections::HashMap;
use std::os::fd::{FromRawFd, RawFd};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};
use xc::container::error::Error;
use xc::container::ContainerManifest;
use xc::errx;
use xc::image_store::sqlite::SqliteImageStore;
use xc::image_store::ImageRecord;
use xc::models::jail_image::{JailConfig, JailImage};
use xc::models::{network::*, MountSpec};

#[usdt::provider]
mod context_provider {
    use crate::ipc::InstantiateRequest;
    fn enter_instantiate(request: &InstantiateRequest) {}
}

pub struct ServerContext {
    pub(crate) sites: HashMap<String, Arc<RwLock<Site>>>,
    pub(crate) terminated_sites: Vec<(String, Arc<RwLock<Site>>)>,

    // map from alias to container id
    pub(crate) alias_map: TwoWayMap<String, String>,

    pub(crate) devfs_store: DevfsRulesetStore,
    pub(crate) image_manager: Arc<RwLock<ImageManager>>,
    pub(crate) config: XcConfig,
    pub(crate) port_forward_table: PortForwardTable,

    // map from id to netgroups
    pub(crate) ng2jails: HashMap<String, Vec<String>>,
    pub(crate) jail2ngs: HashMap<String, Vec<String>>,

    pub(crate) resources: Arc<RwLock<Resources>>,

    pub(crate) ins_queue: HashMap<String, AppliedInstantiateRequest>,
}

impl ServerContext {
    pub(crate) fn new(config: XcConfig) -> ServerContext {
        let image_store_db = SqliteImageStore::open_file(&config.image_database_store);
        image_store_db
            .create_tables()
            .expect("failed to create tables");

        let image_store: Box<SqliteImageStore> = Box::new(image_store_db);

        let db = Arc::new(Database::from(
            rusqlite::Connection::open(&config.database_store)
                .expect("cannot open sqlite database"),
        ));

        db.perform(xc::res::create_tables)
            .expect("cannot create tables");

        let is = Arc::new(Mutex::new(image_store));
        let provider = JsonRegistryProvider::from_path(&config.registries).unwrap();
        let resources = Arc::new(RwLock::new(Resources::new(db, &config)));

        let image_manager = ImageManager::new(
            is,
            &config.image_dataset,
            &config.layers_dir,
            Arc::new(Mutex::new(Box::new(provider))),
        );

        ServerContext {
            alias_map: TwoWayMap::new(),
            devfs_store: DevfsRulesetStore::new(config.devfs_id_offset, config.force_devfs_ruleset),
            image_manager: Arc::new(RwLock::new(image_manager)),
            sites: HashMap::new(),
            terminated_sites: Vec::new(),
            config,
            port_forward_table: PortForwardTable::new(),
            ng2jails: HashMap::new(),
            jail2ngs: HashMap::new(),
            resources,
            ins_queue: HashMap::new(),
        }
    }

    pub(crate) fn config(&self) -> XcConfig {
        self.config.clone()
    }

    pub(crate) async fn inventory(&self) -> crate::config::inventory::Inventory {
        self.resources
            .read()
            .await
            .inventory_manager
            .borrow()
            .clone()
    }

    pub(crate) fn create_channel(
        this: Arc<RwLock<ServerContext>>,
        path: impl AsRef<std::path::Path>,
    ) -> anyhow::Result<tokio::task::JoinHandle<()>> {
        if path.as_ref().exists() {
            std::fs::remove_file(&path)?;
        }
        let mut service = ipc::service::Service::bind(&path, this)?;
        Ok(tokio::spawn(async move {
            crate::ipc::register_to_service(&mut service).await;
            service.start().await;
        }))
    }

    /// Given a pre-computed chain-id of a file system and jail image manifest, register without
    /// checking and verify the chain-id to the database as an image identfied by 'tag' in repo
    /// 'image'
    ///
    /// # Arguments
    /// * `chain_id` - The pre-computed chain-id
    /// * `jail_image` - The container manifest
    /// * `image` - The repo registering to
    /// * `tag` - A tag of the image
    pub(crate) async fn import_fat_tar(
        &mut self,
        diff_id: &oci_util::digest::OciDigest,
        archive: &oci_util::digest::OciDigest,
        content_type: &str,
        meta: &JailConfig,
        image_reference: &ImageReference,
    ) {
        let layers = vec![diff_id.clone()];
        let jail_image = meta.to_image(layers, None);

        _ = self
            .image_manager
            .read()
            .await
            .map_diff_id(diff_id, archive, content_type, None)
            .await;

        _ = self
            .image_manager
            .read()
            .await
            .register_and_tag_manifest(image_reference, &jail_image)
            .await;
    }

    pub async fn resolve_container_by_name(&self, name: &str) -> Option<ContainerManifest> {
        let id = self.alias_map.get(name)?;
        let site = self.sites.get(id)?;
        site.read().await.container_dump()
    }

    pub async fn resolve_container_by_name_nocache(
        &self,
        name: &str,
    ) -> Option<ipc::proto::GenericResult<ContainerManifest>> {
        let id = self.alias_map.get(name)?;
        let site = self.sites.get(id)?;
        let mut site = site.write().await;
        let result = site.query_manifest();
        Some(result)
    }

    pub async fn find_corpse(&self, id: &str) -> Option<ContainerManifest> {
        for (cid, container) in self.terminated_sites.iter().rev() {
            if cid == id {
                return container.read().await.container_dump();
            }
        }
        None
    }

    pub(super) fn get_site(&self, name: &str) -> Option<Arc<RwLock<Site>>> {
        let id = self.alias_map.get(name)?;
        self.sites.get(id).cloned()
    }

    pub(super) async fn update_hosts(&mut self, network: &str) {
        let mut hosts = Vec::new();
        if let Some(jails) = self.ng2jails.get(network) {
            for jail in jails.iter() {
                if let Some(manifest) = self.resolve_container_by_name(jail).await {
                    if let Some(cidr) = manifest.main_ip_for_network(network) {
                        hosts.push((manifest.name.clone(), cidr.addr()));
                    }
                }
            }
            for jail in jails.iter() {
                if let Some(site) = self.get_site(jail) {
                    let mut site = site.write().await;
                    site.update_host_file(network, &hosts);
                }
            }
        }
    }

    // XXX: Potential race condition when trying to import/commit/pull images during purge
    pub(crate) async fn purge_images(&self) -> anyhow::Result<()> {
        info!("begin purge");

        let config = self.config();
        let layers_dir = &config.layers_dir;
        let im = self.image_manager.read().await;
        im.purge().await?;

        let files = std::fs::read_dir(layers_dir).and_then(|dir| {
            let mut files = Vec::new();
            for entry in dir {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    if let Some(filename) = entry
                        .file_name()
                        .to_str()
                        .and_then(|s| s.parse::<OciDigest>().ok())
                    {
                        files.push(filename);
                    }
                }
            }
            Ok(files)
        })?;

        let zfs = ZfsHandle::default();
        let chain_ids = ZfsHandle::default()
            .list_direct_children(&config.image_dataset)?
            .into_iter()
            .filter_map(|pb| {
                pb.file_name()
                    .and_then(|oss| oss.to_str())
                    .and_then(|s| s.parse::<ChainId>().ok())
            });

        let mut file_set: std::collections::HashSet<OciDigest> =
            std::collections::HashSet::from_iter(files);
        let mut chain_id_set: std::collections::HashSet<ChainId> =
            std::collections::HashSet::from_iter(chain_ids);

        let records = im.list_all_tagged().await?;

        for record in records.iter() {
            if !file_set.is_empty() {
                let files = record.manifest.layers();
                for file in files.iter() {
                    for repr in im.query_archives(file).await?.iter() {
                        file_set.remove(&repr.archive_digest);
                        file_set.remove(&repr.diff_id);
                    }
                }
            }
            if !chain_id_set.is_empty() {
                if let Some(cid) = record.manifest.chain_id() {
                    info!("keep: {cid}, wanted by: {}", record.image_reference);
                    chain_id_set.remove(&cid);
                    let props = zfs.get_props(format!("{}/{cid}", config.image_dataset))?;
                    let mut origin_chain = None;
                    while {
                        if let Some(Some(origin)) = props.get("origin") {
                            if let Some(c) = origin
                                .split_once('@')
                                .and_then(|(_, c)| ChainId::from_str(c).ok())
                            {
                                info!("keep: {c}, referenced by {cid}");
                                chain_id_set.remove(&c);
                                origin_chain = Some(c);
                            }
                        }

                        origin_chain.is_some()
                    } {}
                }
            }
        }

        for garbage in file_set.iter() {
            info!("removing orphaned layer: {garbage}");
            let mut layers_dir = layers_dir.clone();
            layers_dir.push(garbage.as_str());
            std::fs::remove_file(layers_dir)?;
        }

        for chain_id in chain_id_set.iter() {
            info!("destroying ZFS dataset: {chain_id}");
            _ = zfs.destroy(
                format!("{}/{chain_id}", config.image_dataset),
                false,
                true,
                false,
            );
        }
        Ok(())
    }

    pub async fn resolve_image(
        &self,
        image_reference: &ImageReference,
    ) -> Result<Option<ImageRecord>, anyhow::Error> {
        match self
            .image_manager
            .read()
            .await
            .query_manifest(image_reference)
            .await
        {
            Err(xc::image_store::ImageStoreError::TagNotFound(_, _)) => Ok(None),
            otherwise => Ok(Some(otherwise?)),
        }
    }

    pub(crate) async fn list_images(
        &self,
        name: impl AsRef<str>,
    ) -> Result<Vec<ImageRecord>, anyhow::Error> {
        Ok(self
            .image_manager
            .read()
            .await
            .query_tags(name.as_ref())
            .await?)
    }

    pub(crate) async fn list_all_images(&self) -> Result<Vec<ImageRecord>, anyhow::Error> {
        Ok(self.image_manager.read().await.list_all_tagged().await?)
    }

    /// Reload the entry pf anchor "xc-rdr"
    pub fn reload_pf_rdr_anchor(&self) -> Result<(), Error> {
        let rules = self.port_forward_table.generate_rdr_rules();
        info!(rules, "reloading xc-rdr anchor");
        if let Err(error) = pf::is_pf_enabled().and_then(|enabled| {
            if enabled {
                pf::set_rules_unchecked(Some("xc-rdr".to_string()), &rules)
            } else {
                warn!("pf is not enabled");
                Ok(())
            }
        }) {
            errx!(EIO, "pfctl error: {error:?}")
        } else {
            Ok(())
        }
    }

    pub(crate) async fn terminate(&mut self, id: &str) -> Result<(), anyhow::Error> {
        if let Some(site) = self.sites.get(id) {
            debug!("sending kill container");
            site.write().await.kill_conatiner()?;
        }
        Ok(())
    }

    pub(crate) async fn destroy_context(&mut self, id: &str) -> Result<(), anyhow::Error> {
        info!("destroy conetxt: {id}");
        if let Some(site) = self.sites.remove(&id.to_string()) {
            if let Err(err) = site.write().await.unwind() {
                error!("error on unwind: {err:#?}");
                return Err(err);
            }

            let mut resources = self.resources.write().await;
            //            let mut nm = self.network_manager.lock().await;

            let addresses = resources
                .release_addresses(id)
                .context("sqlite failure on ip address release")?;

            for (key, addresses) in addresses.iter() {
                let table = format!("xc:network:{key}");
                info!("pf table: {table}");
                if pf::is_pf_enabled().unwrap_or_default() {
                    let result = pf::table_del_addresses(None, &table, addresses);
                    if let Err(error) = result {
                        error!("cannot delete addresses from pf table <{table}>: {error}");
                    }
                }
            }

            self.port_forward_table.remove_rules(id);
            self.reload_pf_rdr_anchor()?;
            self.alias_map.remove_all_referenced(id);

            resources.dataset_tracker.release_container(id);

            if let Some(networks) = self.jail2ngs.get(id) {
                for network in networks.iter() {
                    if let Some(vs) = self.ng2jails.get_mut(network) {
                        if let Some(position) = vs.iter().position(|s| s == id) {
                            vs.remove(position);
                        }
                    }
                }

                self.jail2ngs.remove(id);
            }

            self.terminated_sites.push((id.to_string(), site));
        }
        Ok(())
    }

    pub(crate) async fn list_containers(&self) -> Vec<ContainerManifest> {
        let mut ret = Vec::new();
        for (_, site) in self.sites.iter() {
            ret.push(site.read().await.container_dump().unwrap())
        }
        ret
    }

    pub(crate) async fn do_commit_file(
        &mut self,
        container_name: &str,
        file_fd: RawFd,
    ) -> Result<OciDigest, anyhow::Error> {
        let file = unsafe { std::fs::File::from_raw_fd(file_fd) };
        let site = self.get_site(container_name).context("no surch site")?;
        let mut site = site.write().await;
        let snapshot = site.snapshot_with_generated_tag()?;
        let (diff_id, _digest) = site.commit_to_file("init", &snapshot, file)?;
        Ok(diff_id)
    }

    pub(crate) async fn do_commit(
        &mut self,
        container_name: &str,
        name: &str,
        tag: &str,
    ) -> Result<OciDigest, anyhow::Error> {
        let config = self.config();
        let layers_dir = &config.layers_dir;
        let commit_id = xc::util::gen_id();
        let site = self.get_site(container_name).context("no such site")?;
        let mut site = site.write().await;
        let snapshot = site.snapshot_with_generated_tag()?;
        let temp_file_path = {
            let mut path = layers_dir.clone();
            path.push(commit_id);
            path
        };
        let temp_file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(&temp_file_path)?;
        let (diff_id, digest) = site.commit_to_file("init", &snapshot, temp_file)?;
        // XXX: otherwise default to null image
        let mut image = site.container_dump().and_then(|c| c.origin_image).unwrap();
        image.push_layer(&diff_id);

        let chain_id = image.chain_id().unwrap();
        let dst_dataset = format!("{}/{chain_id}", config.image_dataset);
        site.promote_snapshot(&snapshot, &dst_dataset)?;
        site.zfs.snapshot2(&dst_dataset, "xc")?;

        let image_reference = ImageReference {
            hostname: None,
            name: name.to_string(),
            tag: ImageTag::Tag(tag.to_string()),
        };

        let context = self.image_manager.read().await;
        _ = context
            .register_and_tag_manifest(&image_reference, &image)
            .await;
        context.map_diff_id(&diff_id, &digest, "zstd", None).await?;

        let dest = {
            let mut path = layers_dir.clone();
            path.push(digest.as_str());
            path
        };

        std::fs::rename(&temp_file_path, dest)?;
        Ok(diff_id)
    }

    pub(crate) async fn do_rdr(
        &mut self,
        name: &str,
        rdr: &PortRedirection,
    ) -> Result<(), anyhow::Error> {
        if let Some(container) = self.resolve_container_by_name(name).await {
            if let Some(main_ip) = container.ip_alloc.first() {
                let default_ext_ifs = &self.config.ext_ifs;
                let mut rdr = rdr.clone();
                rdr.with_host_info(default_ext_ifs, main_ip.addresses.first().unwrap().clone());
                self.port_forward_table.append_rule(&container.id, rdr);
                self.reload_pf_rdr_anchor()?;
            }
        }
        Ok(())
    }

    pub(crate) async fn continue_instantiate(
        this: Arc<RwLock<Self>>,
        id: &str,
        applied: AppliedInstantiateRequest,
        cred: Credential,
    ) -> anyhow::Result<()> {
        let no_clean = applied.request.no_clean;
        let name = applied.request.name.clone();

        let (site, notify) = {
            let this = this.clone();
            let mut this = this.write().await;
            let res_ref = this.resources.clone();
            let mut res = res_ref.write().await;
            let mut site = Site::new(id, this.config());
            site.stage(&applied.image)?;
            let netgroups = applied.request.netgroups.clone();

            let blueprint =
                InstantiateBlueprint::new(id, applied, &mut this.devfs_store, &cred, &mut res)?;

            if pf::is_pf_enabled().unwrap_or_default() {
                if let Some(map) = res.get_allocated_addresses(id) {
                    for (network, addresses) in map.iter() {
                        let table = format!("xc:network:{network}");
                        let result = pf::table_add_addresses(None, &table, addresses);
                        if let Err(err) = result {
                            error!("cannot add addresses to <{table}>: {err}");
                        }
                    }
                }
            } else {
                warn!("pf is disabled");
            }

            site.run_container(blueprint)?;
            let notify = site.container_notify.clone().unwrap();
            let jail = site.container_dump().unwrap();
            let arc_site = Arc::new(RwLock::new(site));
            this.sites.insert(id.to_string(), arc_site.clone());
            this.alias_map.insert(id.to_string(), id.to_string());
            this.alias_map.insert(jail.jid.to_string(), id.to_string());
            if let Some(name) = name {
                this.alias_map.insert(name, id.to_string());
            }

            this.jail2ngs
                .insert(id.to_string(), Vec::with_capacity(netgroups.len()));
            for netgroup in netgroups.into_iter() {
                if let Some(jails) = this.ng2jails.get_mut(&netgroup) {
                    jails.push(id.to_string());
                } else {
                    this.ng2jails
                        .insert(netgroup.to_string(), vec![id.to_string()]);
                }
                this.jail2ngs.get_mut(id).unwrap().push(netgroup.clone());
                this.update_hosts(&netgroup).await;
            }

            (arc_site, notify)
        };

        let id = id.to_string();

        tokio::spawn(async move {
            let arc_site = site;
            {
                let arc_site = arc_site.clone();
                debug!("main_started_notify started await");
                let notify = arc_site.read().await.main_notify.clone().unwrap();
                debug!("main_started_notify exited await");
                _ = notify.notified().await;
            };
            {
                let arc_site = arc_site.clone();
                arc_site.read().await.notify_main_started();
            }
        });

        tokio::spawn(async move {
            debug!("destroy_context notify started await");
            _ = notify.notified().await;
            debug!("destroy_context notify exited await");

            let mut this = this.write().await;
            {
                if !no_clean {
                    _ = this.destroy_context(&id).await;
                }
            }
        });

        Ok(())
    }

    /// Create a new site with id and instantiate a container in the site
    pub(crate) async fn instantiate(
        this: Arc<RwLock<Self>>,
        id: &str,
        image: &JailImage,
        request: InstantiateRequest,
        cred: Credential,
    ) -> anyhow::Result<Vec<String>> {
        context_provider::enter_instantiate!(|| &request);

        let applied = {
            let this = this.clone();
            let this = this.write().await;
            let resources_ref = this.resources.clone();
            let mut resources = resources_ref.write().await;

            AppliedInstantiateRequest::new(request, image, &cred, &mut resources)?
        };
        if !applied.devfs_rules.is_empty() {
            let rules = applied.devfs_rules.iter().map(|r| r.to_string()).collect();
            this.write().await.ins_queue.insert(id.to_string(), applied);
            Ok(rules)
        } else {
            Self::continue_instantiate(this, id, applied, cred).await?;
            Ok(Vec::new())
        }
    }

    pub(crate) async fn push_image(
        &self,
        reference: ImageReference,
        remote_reference: ImageReference,
    ) -> Result<(), crate::image::push::PushImageError> {
        _ = crate::image::push::push_image(
            self.image_manager.clone(),
            &self.config.layers_dir,
            reference,
            remote_reference,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn pull_image(
        &mut self,
        reference: ImageReference,
        rename_reference: Option<ImageReference>,
    ) -> Result<(), PullImageError> {
        // XXX: handle pull image error
        let result =
            crate::image::pull::pull_image(self.image_manager.clone(), reference, rename_reference)
                .await;
        if result.is_err() {
            error!("result: {result:#?}");
        }
        result.map(|_| ())
    }

    pub(crate) async fn list_volumes(&self) -> HashMap<String, Volume> {
        self.resources.read().await.list_volumes()
    }

    pub(crate) async fn create_volume(
        &mut self,
        name: &str,
        template: Option<MountSpec>,
        kind: VolumeDriverKind,
        source: Option<std::path::PathBuf>,
        zfs_props: HashMap<String, String>,
    ) -> Result<(), Error> {
        self.resources
            .write()
            .await
            .create_volume(kind, name, template, source, zfs_props)
    }
}
