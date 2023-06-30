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
pub mod instantiate;

use crate::auth::Credential;
use crate::config_manager::ConfigManager;
use crate::context::instantiate::InstantiateBlueprint;
use crate::devfs_store::DevfsRulesetStore;
use crate::image::ImageManager;
use crate::ipc::InstantiateRequest;
use crate::network_manager::NetworkManager;
use crate::port::PortForwardTable;
use crate::registry::JsonRegistryProvider;
use crate::site::Site;

use anyhow::Context;
use freebsd::fs::zfs::{ZfsHandle, ZfsSnapshot};
use oci_util::digest::OciDigest;
use oci_util::image_reference::ImageReference;
use oci_util::layer::ChainId;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info};
use xc::config::XcConfig;
use xc::container::ContainerManifest;
use xc::image_store::sqlite::SqliteImageStore;
use xc::image_store::ImageRecord;
use xc::models::jail_image::{JailConfig, JailImage};
use xc::models::network::*;

pub struct ServerContext {
    pub(crate) network_manager: Arc<Mutex<NetworkManager>>,
    pub(crate) sites: HashMap<String, Arc<RwLock<Site>>>,
    pub(crate) alias_map: HashMap<String, String>,
    pub(crate) devfs_store: DevfsRulesetStore,
    pub(crate) image_manager: Arc<RwLock<ImageManager>>,
    pub(crate) config_manager: ConfigManager,
    pub(crate) port_forward_table: PortForwardTable,
}

impl ServerContext {
    pub(crate) fn new(config_manager: ConfigManager) -> ServerContext {
        let config = config_manager.config();
        let image_store_db = SqliteImageStore::open_file(&config.image_database_store);
        image_store_db
            .create_tables()
            .expect("failed to create tables");

        let image_store: Box<SqliteImageStore> = Box::new(image_store_db);
        let db = rusqlite::Connection::open(&config.database_store)
            .expect("cannot open sqlite database");

        xc::res::create_tables(&db).expect("cannot create tables");

        let network_manager = Arc::new(Mutex::new(NetworkManager::new(
            db,
            config_manager.subscribe(),
        )));
        let is = Arc::new(Mutex::new(image_store));
        let provider = JsonRegistryProvider::from_path(&config.registries).unwrap();

        let image_manager = ImageManager::new(
            is,
            &config.image_dataset,
            &config.layers_dir,
            Arc::new(Mutex::new(Box::new(provider))),
        );

        ServerContext {
            network_manager,
            alias_map: HashMap::new(),
            devfs_store: DevfsRulesetStore::new(config.devfs_id_offset),
            image_manager: Arc::new(RwLock::new(image_manager)),
            sites: HashMap::new(),
            config_manager,
            port_forward_table: PortForwardTable::new(),
        }
    }

    pub(crate) fn config(&self) -> XcConfig {
        self.config_manager.config()
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
        image: &str,
        tag: &str,
    ) {
        let layers = vec![diff_id.clone()];
        let jail_image = meta.to_image(layers);

        _ = self
            .image_manager
            .read()
            .await
            .map_diff_id(diff_id, archive, content_type)
            .await;

        _ = self
            .image_manager
            .read()
            .await
            .register_and_tag_manifest(image, tag, &jail_image)
            .await;
    }

    pub async fn resolve_container_by_name(&self, name: &str) -> Option<ContainerManifest> {
        let id = self.alias_map.get(name)?;
        let site = self.sites.get(id)?;
        site.read().await.container_dump()
    }

    pub(super) fn get_site(&self, name: &str) -> Option<Arc<RwLock<Site>>> {
        let id = self.alias_map.get(name)?;
        self.sites.get(id).cloned()
    }

    pub async fn resolve_image(
        &self,
        name: impl AsRef<str>,
        reference: impl AsRef<str>,
    ) -> Result<Option<ImageRecord>, anyhow::Error> {
        match self
            .image_manager
            .read()
            .await
            .query_manifest(name.as_ref(), reference.as_ref())
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
    pub fn reload_pf_rdr_anchor(&self) -> Result<(), std::io::Error> {
        let rules = self.port_forward_table.generate_rdr_rules();
        info!(rules, "reloading xc-rdr anchor");
        _ = freebsd::net::pf::set_rules_unchecked(Some("xc-rdr".to_string()), &rules);
        Ok(())
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
            let nm = self.network_manager.lock().await;
            nm.release_addresses(id)
                .context("sqlite failure on ip address release")?;
            self.port_forward_table.remove_rules(id);
            self.reload_pf_rdr_anchor()?;
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

    pub(crate) async fn do_commit(
        &mut self,
        container_name: &str,
        name: &str,
        tag: &str,
    ) -> Result<String, anyhow::Error> {
        let container = self
            .resolve_container_by_name(container_name)
            .await
            .expect("no such container");
        let container_id = container.id.to_string();
        let commit_id = xc::util::gen_id();
        let config = self.config();
        let layers_dir = &config.layers_dir;
        let temp_file = format!("{layers_dir}/{commit_id}");

        let running_dataset = format!("{}/{}", config.container_dataset, container_id);
        let dst_dataset = format!("{}/{}", config.container_dataset, commit_id);

        let zfs_origin = container.zfs_origin.clone().expect("missing zfs origin");

        let zfs = ZfsHandle::default();
        let mut manifest = container
            .origin_image
            .clone()
            .expect("missing origin image");

        {
            zfs.snapshot2(running_dataset.clone(), &commit_id.to_string())
                .unwrap();
            zfs.clone2(
                running_dataset.clone(),
                &commit_id.to_string(),
                dst_dataset.clone(),
            )
            .unwrap();
            zfs.promote(dst_dataset.clone()).unwrap();
        }
        let snapshot = ZfsSnapshot::new(&dst_dataset, "xc");
        debug!("taking zfs snapshot for {dst_dataset}@xc");
        snapshot.execute(&zfs)?;

        let prev_chain_id = zfs_origin.rsplit_once('/').expect("").1;
        debug!("prev_chain_id: {prev_chain_id:#?}");
        let mut chain_id = ChainId::from_str(prev_chain_id)?;

        let output = std::process::Command::new("ocitar")
            .arg("-cf")
            .arg(temp_file.clone())
            .arg("--zfs-diff")
            .arg(format!("{zfs_origin}@xc"))
            .arg(format!("{dst_dataset}@xc"))
            .output()
            .expect("fail to spawn ocitar");

        let diff_id = {
            let diff_id = std::str::from_utf8(&output.stdout).unwrap().trim();
            eprintln!("diff_id: {diff_id}");
            eprintln!("rename: {temp_file} -> {}/{diff_id}", config.layers_dir);
            std::fs::rename(temp_file, format!("{}/{diff_id}", config.layers_dir))?;
            OciDigest::from_str(diff_id).unwrap()
        };

        chain_id.consume_diff_id(oci_util::digest::DigestAlgorithm::Sha256, &diff_id);
        let new_name = format!("{}/{chain_id}", config.image_dataset);
        zfs.rename(&dst_dataset, new_name)?;

        manifest.push_layer(&diff_id);

        let context = self.image_manager.read().await;

        _ = context
            .register_and_tag_manifest(name, tag, &manifest)
            .await;

        context.map_diff_id(&diff_id, &diff_id, "plain").await?;

        Ok(commit_id)
    }

    pub(crate) async fn do_rdr(
        &mut self,
        name: &str,
        rdr: &PortRedirection,
    ) -> Result<(), anyhow::Error> {
        if let Some(container) = self.resolve_container_by_name(name).await {
            if let Some(main_ip) = container.ip_alloc.first() {
                let default_ext_ifs = self.config_manager.config().ext_ifs;
                let mut rdr = rdr.clone();
                rdr.with_host_info(&default_ext_ifs, main_ip.addresses.first().unwrap().clone());
                self.port_forward_table.append_rule(&container.id, rdr);
                self.reload_pf_rdr_anchor()?;
            }
        }
        Ok(())
    }

    /// Create a new site with id and instantiate a container in the site
    pub(crate) async fn instantiate(
        this: Arc<RwLock<Self>>,
        id: &str,
        image: &JailImage,
        request: InstantiateRequest,
        cred: Credential,
    ) -> anyhow::Result<()> {
        let no_clean = request.no_clean;

        let (site, notify) = {
            let this = this.clone();
            let mut this = this.write().await;
            let mut site = Site::new(id, this.config_manager.subscribe());
            site.stage(image)?;
            let name = request.name.clone();
            let blueprint = {
                let network_manager = this.network_manager.clone();
                let network_manager = network_manager.lock().await;
                InstantiateBlueprint::new(
                    id,
                    image,
                    request,
                    &mut this.devfs_store,
                    &cred,
                    &network_manager,
                )?
            };

            site.run_container(blueprint)?;
            let notify = site.container_notify.clone().unwrap();
            let arc_site = Arc::new(RwLock::new(site));
            this.sites.insert(id.to_string(), arc_site.clone());
            this.alias_map.insert(id.to_string(), id.to_string());
            if let Some(name) = name {
                this.alias_map.insert(name, id.to_string());
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

    pub(crate) async fn push_image(
        &self,
        reference: ImageReference,
        remote_reference: ImageReference
    ) -> Result<(), crate::image::PushImageError> {
        _ = ImageManager::push_image(
            self.image_manager.clone(),
            &self.config_manager.config().layers_dir,
            reference,
            remote_reference
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn pull_image(&mut self, reference: &ImageReference) -> anyhow::Result<()> {
        ImageManager::pull_image(self.image_manager.clone(), reference.clone()).await;
        Ok(())
    }
}
