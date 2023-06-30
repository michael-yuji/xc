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

use crate::registry::*;
use crate::task::*;
use freebsd::fs::zfs::{ZfsError, ZfsHandle};
use oci_util::digest::{DigestAlgorithm, Hasher, OciDigest};
use oci_util::distribution::client::*;
use oci_util::image_reference::ImageReference;
use oci_util::layer::ChainId;
use oci_util::models::{Descriptor, ImageManifest};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::watch::Receiver;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info};
use xc::image_store::sqlite::SqliteImageStore;
use xc::image_store::{DiffIdMap, ImageRecord, ImageStore, ImageStoreError};
use xc::models::jail_image::{JailConfig, JailImage};
use xc::tasks::{DownloadLayerStatus, ImportImageState, ImportImageStatus};
use xc::util::*;

#[derive(Error, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PushImageError {
    #[error("no such local reference")]
    NoSuchLocalReference,
    #[error("requested registry not found")]
    RegistryNotFound,
}

struct DiffMap {
    diff_id: OciDigest,
    descriptor: Descriptor,
}

/// Shared environment accessible to workers
#[derive(Clone)]
struct SharedContext {
    image_store: Arc<tokio::sync::Mutex<Box<SqliteImageStore>>>,
    registries: Arc<tokio::sync::Mutex<Box<dyn RegistriesProvider + Sync + Send>>>,
    image_dataset: PathBuf,
    layers_dir: PathBuf,
}

impl SharedContext {
    fn new(
        image_store: Arc<tokio::sync::Mutex<Box<SqliteImageStore>>>,
        image_dataset: impl AsRef<Path>,
        layers_dir: impl AsRef<Path>,
        registries: Arc<Mutex<Box<dyn RegistriesProvider + Send + Sync>>>,
    ) -> SharedContext {
        SharedContext {
            image_store,
            image_dataset: image_dataset.as_ref().to_path_buf(),
            layers_dir: layers_dir.as_ref().to_path_buf(),
            registries,
        }
    }
}

pub struct ImageManager {
    context: SharedContext,
    layers: NotificationStore<SharedContext, OciDigest, PullLayerStatus>,
    rootfs: NotificationStore<SharedContext, ChainId, StageRootStatus>,
    images: NotificationStore<SharedContext, String, PullImageStatus>,
    push_image: NotificationStore<SharedContext, String, PushImageStatus>,
}

impl ImageManager {
    pub fn new(
        image_store: Arc<Mutex<Box<SqliteImageStore>>>,
        image_dataset: impl AsRef<Path>,
        layers_dir: impl AsRef<Path>,
        registries: Arc<Mutex<Box<dyn RegistriesProvider + Sync + Send>>>,
    ) -> ImageManager {
        let shared_context = SharedContext::new(image_store, image_dataset, layers_dir, registries);
        ImageManager {
            layers: NotificationStore::new(shared_context.clone()),
            rootfs: NotificationStore::new(shared_context.clone()),
            images: NotificationStore::new(shared_context.clone()),
            push_image: NotificationStore::new(shared_context.clone()),
            context: shared_context,
        }
    }

    pub async fn register_and_tag_manifest(
        &self,
        name: &str,
        tag: &str,
        manifest: &JailImage,
    ) -> Result<OciDigest, ImageStoreError> {
        self.context
            .image_store
            .lock()
            .await
            .register_and_tag_manifest(name, tag, manifest)
    }

    pub async fn query_manifest(
        &self,
        name: &str,
        tag: &str,
    ) -> Result<ImageRecord, ImageStoreError> {
        self.context
            .image_store
            .lock()
            .await
            .query_manifest(name, tag)
    }

    pub async fn query_tags(&self, name: &str) -> Result<Vec<ImageRecord>, ImageStoreError> {
        self.context.image_store.lock().await.list_all_tags(name)
    }

    pub async fn list_all_tagged(&self) -> Result<Vec<ImageRecord>, ImageStoreError> {
        self.context.image_store.lock().await.list_all_tagged()
    }

    #[allow(dead_code)]
    pub async fn query_records_using_commit(
        &self,
        commit_id: &str,
    ) -> Result<Vec<ImageRecord>, ImageStoreError> {
        self.context
            .image_store
            .lock()
            .await
            .query_records_using_commit(commit_id)
    }

    pub async fn map_diff_id(
        &self,
        diff_id: &OciDigest,
        archive: &OciDigest,
        content_type: &str,
    ) -> Result<(), ImageStoreError> {
        self.context
            .image_store
            .lock()
            .await
            .map_diff_id(diff_id, archive, content_type)
    }

    #[allow(dead_code)]
    pub async fn associate_commit_manifest(
        &self,
        commit_id: &str,
        manifest: &JailImage,
    ) -> Result<(), ImageStoreError> {
        self.context
            .image_store
            .lock()
            .await
            .associate_commit_manifest(commit_id, manifest)
    }

    pub async fn query_archives(
        &self,
        diff_id: &OciDigest,
    ) -> Result<Vec<DiffIdMap>, ImageStoreError> {
        self.context
            .image_store
            .lock()
            .await
            .query_archives(diff_id)
    }

    pub fn get_upload_state(&mut self, id: &str) -> PushImageStatusDesc {
        match self.push_image.get(&id.to_string()) {
            None => PushImageStatusDesc::default(),
            Some(value) => {
                let mut status = value.borrow().last_state.clone();
                status.fault = value.borrow().fault();
                status.to_desc()
            }
        }
    }

    /// Given an id representing a import image task, try query its download status. If the
    /// task is on-going, walk through the sub-task required by this download task and report
    /// the status of each
    ///
    pub fn get_download_state(&mut self, id: &str) -> ImportImageStatus {
        match self.images.get(&id.to_string()) {
            None => ImportImageStatus::unavailable(),
            Some(value) => {
                // If the value is in the images notification store, this means we have
                // ended the end state of the state machine, which means either downloaded
                // the image, or we encountered a fault
                let completed = value.borrow().is_completed();
                let val = value.borrow().last_state.clone();

                let mut status = ImportImageStatus {
                    manifest: val.manifest,
                    config: val.config,
                    fault: value.borrow().fault(),
                    ..ImportImageStatus::default()
                };

                let state = if completed {
                    ImportImageState::Done
                } else if status.config.is_some() {
                    let mut dls = Vec::new();
                    for desc in status.manifest.clone().unwrap().layers.iter() {
                        if let Some(c) = self.layers.get(&desc.digest) {
                            let t = c.borrow().is_completed();
                            let v = c.borrow().last_state.clone();
                            if !t {
                                dls.push(DownloadLayerStatus {
                                    digest: desc.digest.clone(),
                                    downloaded: v.written,
                                    total: Some(v.total),
                                })
                            }
                        }
                    }
                    if dls.is_empty() {
                        if completed {
                            ImportImageState::Done
                        } else {
                            ImportImageState::ExtractLayers
                        }
                    } else {
                        status.layers = Some(dls);
                        ImportImageState::DownloadLayers
                    }
                } else if status.manifest.is_some() {
                    ImportImageState::DownloadConfig
                } else {
                    ImportImageState::DownloadManifest
                };

                status.state = state;
                status
            }
        }
    }

    fn get_layer(
        &mut self,
        mut session: Session,
        descriptor: Descriptor,
    ) -> Receiver<Task<OciDigest, PullLayerStatus>> {
        let digest = descriptor.digest.clone();
        if let Some(rx) = self.layers.get(&descriptor.digest) {
            rx
        } else {
            let (mut emitter, rx) = self.layers.register(&descriptor.digest);
            let context = self.context.clone();

            if !emitter.is_completed() {
                _ = emitter.use_try(|state| {
                    state.total = descriptor.size;
                    Ok(())
                });

                tokio::spawn(async move {
                    let mut emitter = emitter;
                    let target_path = {
                        let mut parent = context.layers_dir.clone();
                        parent.push(format!("{digest}"));
                        parent
                    };

                    let in_progress_path = {
                        let mut parent = context.layers_dir.clone();
                        parent.push(format!("{digest}.progress"));
                        parent
                    };

                    macro_rules! use_state {
                        ($f:expr) => {{
                            let res = emitter.use_try($f);
                            if res.is_err() {
                                return;
                            }
                            res.unwrap()
                        }};
                    }

                    if let Ok(mut response) = session.fetch_blob(&digest).await {
                        let in_progress_path_string =
                            in_progress_path.to_string_lossy().to_string();
                        let mut hasher = Hasher::new(digest.algorithm());

                        let (cat, format) = if descriptor.media_type.ends_with("gzip") {
                            ("gzcat", "gzip")
                        } else if descriptor.media_type.ends_with("zstd") {
                            ("zstdcat", "zstd")
                        } else {
                            ("cat", "plain")
                        };

                        let shell_script =
                            format!("tee {in_progress_path_string} | {cat} - | sha256 -q");

                        let mut helper = Command::new("sh")
                            .arg("-c")
                            .arg(shell_script)
                            .stdout(Stdio::piped())
                            .stdin(Stdio::piped())
                            .spawn()
                            .expect("cannot spawn sh helper");

                        let stdin = helper.stdin.as_mut().unwrap();

                        while let Ok(Some(chunk)) = response.chunk().await {
                            hasher.update(&chunk);
                            stdin.write_all(&chunk).unwrap();
                            //                            file.write_all(&chunk).unwrap();
                            use_state!(|state| {
                                state.written += chunk.len();
                                Ok(())
                            });
                        }

                        let output = helper.wait_with_output().unwrap();

                        let diff_id = {
                            let string = std::str::from_utf8(&output.stdout).unwrap().trim();
                            format!("sha256:{string}")
                        };

                        info!("get_layer: dffid={diff_id}");
                        let digest = hasher.finalize();
                        let diff_id = OciDigest::from_str(&diff_id).unwrap();

                        context
                            .image_store
                            .lock()
                            .await
                            .map_diff_id(&diff_id, &digest, format)
                            .unwrap();

                        use_state!(|_| std::fs::rename(&in_progress_path, &target_path)
                            .map_err(|_| "failed to mv file".to_string()));
                        emitter.set_completed()
                    }
                });
            }

            rx
        }
    }

    fn stage_root(
        &mut self,
        session: &Session,
        chain_id: &ChainId,
        diff_maps: &[DiffMap], //        descriptors: &[Descriptor],
    ) -> Receiver<Task<ChainId, StageRootStatus>> {
        if let Some(rx) = self.rootfs.get(chain_id) {
            rx
        } else {
            let (mut emitter, rx) = self.rootfs.register(chain_id);

            if !emitter.is_completed() {
                let dataset = self.context.image_dataset.clone();
                let existing = zfs_list_chain_ids(dataset);

                if existing.contains(chain_id) {
                    emitter.set_completed();
                    return rx;
                }

                let recipe = RootFsRecipe::resolve(&existing, diff_maps);

                let mut layers = Vec::with_capacity(recipe.digests.len());

                for digest in recipe.digests.iter() {
                    let layer = self.get_layer(session.clone(), digest.clone());
                    if !(*layer.borrow()).is_completed() {
                        layers.push(layer);
                    }
                }

                let context = self.context.clone();

                tokio::spawn(async move {
                    let mut emitter = emitter;
                    let n2 = layers
                        .iter()
                        .map(|r| r.borrow().notify.clone())
                        .collect::<Vec<_>>();

                    let notifies = n2.iter().map(|r| r.notified());
                    futures::future::join_all(notifies).await;

                    let faults = layers.iter().filter_map(|s| (*s.borrow()).fault());
                    if let Some(reason) = faults.reduce(|a, b| format!("{a}\n{b}")) {
                        emitter.set_faulted(&reason);
                    } else if let Err(reason) = recipe
                        .stage_layers_assume_existed(&context.image_dataset, &context.layers_dir)
                        .await
                    {
                        emitter.set_faulted(&format!("{reason:?}"));
                    } else {
                        emitter.set_completed();
                    }
                });
            }
            rx
        }
    }

    pub async fn push_image(
        this: Arc<RwLock<Self>>,
        layers_dir: &str,
//        registry: &str,
        reference: ImageReference,
        remote_reference: ImageReference,
//        name: &str,
//        tag: &str,
    ) -> Result<Receiver<Task<String, PushImageStatus>>, PushImageError> {

//        let id = reference.to_string();
        let id = format!("{reference}->{remote_reference}");
        let name = remote_reference.name;
        let tag = remote_reference.tag.to_string();

        let (registry, record) = {
            let this = this.clone();
            let this = this.read().await;
            let reg = this.context.registries.lock().await;

            let record = this
                .query_manifest(&reference.name, reference.tag.as_ref())
                .await
                .map_err(|_| PushImageError::NoSuchLocalReference)?;

            let registry = remote_reference.hostname
                .and_then(|registry| reg.get_registry_by_name(&registry))
                .ok_or(PushImageError::RegistryNotFound)?;

            (registry, record)
        };

        let maybe_task = { this.clone().write().await.push_image.get(&id) };
        if let Some(task_receiver) = maybe_task {
            if !(*task_receiver.borrow()).has_failed() {
                return Ok(task_receiver);
            }
        }

        let (mut emitter, rx) = { this.clone().write().await.push_image.register(&id) };
        let name = name.to_string();
        let tag = tag.to_string();
        let layers_dir = layers_dir.to_string();

        tokio::spawn(async move {
            let this = this.clone();
            let mut session = registry.new_session(name.to_string());
            let layers = record.manifest.layers().clone();
            let mut selections = Vec::new();
//            let (tx, upload_status) = tokio::sync::watch::channel(UploadStat::default());

            'layer_loop: for layer in layers.iter() {
                let maps = {
                    let this = this.clone();
                    let this = this.read().await;
                    this.query_archives(layer).await?
                };

                for map in maps.iter() {
                    let layer_file = format!("{}/{}", layers_dir, map.archive_digest);
                    let layer_file_path = std::path::Path::new(&layer_file);
                    if layer_file_path.exists() {
                        selections.push(map.clone());
                        continue 'layer_loop;
                    }
                }

                // XXX: we actually can recover by generating those layers, just not in this version
                {
                    tracing::error!("cannot find archive layer for {layer}");
                    emitter.set_faulted(&format!("cannot find archive layer for {layer}"));
                    anyhow::bail!("cannot find archive layer for {layer}");
                }

                break;
            }

            _ = emitter.use_try(|state| {
                state.layers = layers.clone();
                Ok(())
            });

            if layers.len() != selections.len() {
                todo!()
            }

            let mut uploads = Vec::new();
            for map in selections.iter() {
                let content_type = match map.algorithm.as_str() {
                    "gzip" => "application/vnd.oci.image.layer.v1.tar+gzip",
                    "zstd" => "application/vnd.oci.image.layer.v1.tar+zstd",
                    "plain" => "application/vnd.oci.image.layer.v1.tar",
                    _ => unreachable!(),
                };

                let path = format!("{layers_dir}/{}", map.archive_digest);
                let path = std::path::Path::new(&path);
                let file = std::fs::OpenOptions::new().read(true).open(path)?;

//                let dedup_check = Ok::<bool, std::io::Error>(false);
                let dedup_check = session.exists_digest(&map.archive_digest).await;

                _ = emitter.use_try(|state| {
                    if state.current_upload.is_none() {
                        state.current_upload = Some(0);
                    }
                    Ok(())
                });
                let descriptor = if dedup_check.is_ok() && dedup_check.unwrap() {
                    let metadata = file.metadata().unwrap();
                    _ = emitter.use_try(|state| {
                        if state.current_upload.is_none() {
                            state.current_upload = Some(0);
                        }
                        Ok(())
                    });

                    Descriptor {
                        digest: map.archive_digest.clone(),
                        media_type: content_type.to_string(),
                        size: metadata.len() as usize,
                    }
                } else {
                    info!("pushing {path:?}");
                    let (tx, upload_status) = tokio::sync::watch::channel(UploadStat::default());
                    _ = emitter.use_try(|state| {
                        state.upload_status = Some(upload_status.clone());
                        Ok(())
                    });

                    let maybe_descriptor = session
                        .upload_content(Some(tx), content_type.to_string(), file)
                        .await;
                    maybe_descriptor?
                };
                uploads.push(descriptor);
                _ = emitter.use_try(|state| {
                    state.current_upload = Some(state.current_upload.unwrap() + 1);
                    Ok(())
                });
            }
            // upload config
            let config = serde_json::to_vec(&record.manifest)?;
            _ = emitter.use_try(|state| {
                state.push_config = true;
                Ok(())
            });
            debug!("pushing config");
            let config_descriptor = session
                .upload_content(
                    None,
                    "application/vnd.oci.image.config.v1+json".to_string(),
                    config.as_slice(),
                )
                .await?;
            let manifest = oci_util::models::ImageManifest {
                schema_version: 2,
                media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
                config: config_descriptor, //,config.unwrap(),
                layers: uploads,
            };

            debug!("Registering manifest: {manifest:#?}");
            _ = emitter.use_try(|state| {
                state.push_manifest = true;
                Ok(())
            });
            session.register_manifest(&tag, &manifest).await?;

            debug!("Registering manifest: {manifest:#?}");
            _ = emitter.use_try(|state| {
                state.done = true;
                Ok(())
            });
            emitter.set_completed();

            Ok::<(), anyhow::Error>(())
        });

        Ok(rx)
    }

    pub async fn pull_image(
        this: Arc<RwLock<Self>>,
        reference: ImageReference,
    ) -> Receiver<Task<String, PullImageStatus>> {
        let id = reference.to_string();

        let image = reference.name.clone();
        let tag = match reference.tag {
            oci_util::image_reference::ImageTag::Tag(tag) => tag,
            oci_util::image_reference::ImageTag::Digest(digest) => digest.to_string(),
        };

        let registry = {
            let this = this.clone();
            let this = this.read().await;
            let reg = this.context.registries.lock().await;
            match reference.hostname {
                None => reg.default_registry().expect("no default registry found"),
                Some(name) => reg.get_registry_by_name(&name).expect("no such registry"),
            }
        };

        let maybe_task = { this.clone().write().await.images.get(&id) };

        if let Some(task_receiver) = maybe_task {
            if !(*task_receiver.borrow()).has_failed() {
                return task_receiver;
            }
        }

        let (mut emitter, rx) = { this.clone().write().await.images.register(&id) };
        macro_rules! ty {
            ($f:expr) => {{
                match $f {
                    Ok(res) => res,
                    Err(err) => {
                        emitter.set_faulted(&format!("{err:?}"));
                        return;
                    }
                }
            }};
        }
        if !emitter.is_completed() {
            tokio::spawn(async move {
                let mut session = registry.new_session(image.to_string());
                let manifest = session
                    .query_manifest_traced(&tag, |list| {
                        list.manifests
                            .iter()
                            .find(|desc| desc.platform.architecture == get_current_arch())
                            .cloned()
                    })
                    .await;

                match manifest {
                    Ok(Some(manifest)) => {
                        _ = emitter.use_try(|state| {
                            state.manifest = Some(manifest.clone());
                            Ok(())
                        });

                        let config_descriptor = manifest.config.clone();

                        let config: Result<Option<serde_json::Value>, ClientError> =
                            session.fetch_blob_as(&config_descriptor.digest).await;

                        if let Ok(Some(config)) = config.as_ref() {
                            _ = emitter.use_try(|state| {
                                state.config = Some(config.clone());
                                Ok(())
                            });
                        }

                        let jail_image = config.map(|a| a.and_then(JailConfig::from_json));

                        match jail_image {
                            Ok(Some(jail_image)) => {
                                _ = emitter.use_try(|state| {
                                    state.jail_image = Some(jail_image.clone());
                                    Ok(())
                                });

                                match jail_image.chain_id() {
                                    Some(chain_id) => {
                                        let wait_stage_root = {
                                            let diff_maps = manifest
                                                .layers
                                                .iter()
                                                .zip(jail_image.layers().iter())
                                                .map(|(descriptor, diff_id)| DiffMap {
                                                    descriptor: descriptor.clone(),
                                                    diff_id: diff_id.clone(),
                                                })
                                                .collect::<Vec<_>>();
                                            this.clone()
                                                .write()
                                                .await
                                                .stage_root(&session, &chain_id, &diff_maps)
                                        };

                                        let notify = {
                                            let x = wait_stage_root.borrow();
                                            if x.is_completed() {
                                                None
                                            } else {
                                                Some(x.notify.clone())
                                            }
                                        };

                                        if let Some(notify) = notify {
                                            notify.notified().await;
                                        }

                                        let fault = (*wait_stage_root.borrow()).fault();
                                        if let Some(reason) = fault {
                                            emitter.set_faulted(&format!(
                                                "failed to stage root: {reason}"
                                            ));
                                        } else {
                                            {
                                                let this = this.clone();
                                                let arc_image_store =
                                                    &this.write().await.context.image_store;
                                                let image_store = arc_image_store.lock().await;
                                                ty!(image_store
                                                    .register_and_tag_manifest(
                                                        &image,
                                                        &tag,
                                                        &jail_image
                                                    )
                                                    .map_err(|err| format!("{err:?}")));
                                                emitter.set_completed();
                                            }
                                            emitter.set_completed();
                                        }
                                    }
                                    None => {
                                        emitter.set_completed();
                                    }
                                }
                            }
                            Ok(None) => emitter.set_faulted("cannot find config"),
                            Err(err) => {
                                emitter.set_faulted(&format!("failed request for config: {err:?}"))
                            }
                        }
                    }
                    Ok(None) => emitter.set_faulted("cannot find manifest"),
                    Err(err) => emitter.set_faulted(&format!("failed request manifest: {err:?}")),
                }
            });
        }

        rx
    }
}

impl FromId<SharedContext, OciDigest> for PullLayerStatus {
    fn from_id(context: SharedContext, k: &OciDigest) -> (Self, TaskStatus) {
        let mut file_path = context.layers_dir;
        file_path.push(k.as_str());
        if file_path.exists() {
            (
                PullLayerStatus {
                    written: 0,
                    total: 0,
                },
                TaskStatus::Completed,
            )
        } else {
            (
                PullLayerStatus {
                    written: 0,
                    total: 0,
                },
                TaskStatus::InProgress,
            )
        }
    }
}

impl FromId<SharedContext, ChainId> for StageRootStatus {
    fn from_id(context: SharedContext, k: &ChainId) -> (Self, TaskStatus) {
        let dataset = context.image_dataset.to_string_lossy().to_string();
        let zfs = ZfsHandle::default();
        let dset = format!("{dataset}/{k}@xc");
        if zfs.exists(dset) {
            //        if freebsd::fs::zfs::exist_dataset(format!("{dataset}/{k}@xc")) {
            (StageRootStatus {}, TaskStatus::Completed)
        } else {
            (StageRootStatus {}, TaskStatus::InProgress)
        }
    }
}

impl FromId<SharedContext, String> for PullImageStatus {
    fn from_id(_context: SharedContext, _k: &String) -> (Self, TaskStatus) {
        let image_status = PullImageStatus {
            config: None,
            manifest: None,
            jail_image: None, //            layers: None,
        };
        (image_status, TaskStatus::InProgress)
    }
}

impl FromId<SharedContext, String> for PushImageStatus {
    fn from_id(_context: SharedContext, _k: &String) -> (Self, TaskStatus) {
        let status = PushImageStatus::default();
        (status, TaskStatus::InProgress)
    }
}

#[derive(Clone, Default, Debug, Deserialize, Serialize)]
pub struct PullLayerStatus {
    written: usize,
    total: usize,
}

#[derive(Clone, Default, Debug)]
pub struct StageRootStatus {}

#[derive(Clone, Debug)]
pub struct PullImageStatus {
    manifest: Option<ImageManifest>,
    config: Option<serde_json::Value>,
    jail_image: Option<xc::models::jail_image::JailImage>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PushImageStatusDesc {
    pub layers: Vec<OciDigest>,
    pub current_upload: Option<usize>,
    pub push_config: bool,
    pub push_manifest: bool,
    pub done: bool,
    pub fault: Option<String>,

    pub bytes: Option<usize>,
    pub duration_secs: Option<u64>
}

#[derive(Clone, Default)]
pub struct PushImageStatus {
    pub layers: Vec<OciDigest>,
    /// the layer we are pushing, identify by the position of it in the layers stack
    pub current_upload: Option<usize>,
    pub push_config: bool,
    pub push_manifest: bool,
    pub done: bool,
    pub fault: Option<String>,
    pub upload_status: Option<Receiver<UploadStat>>
}

impl PushImageStatus {
    fn to_desc(&self) -> PushImageStatusDesc {
        let (bytes, duration_secs) = match &self.upload_status {
            None => {
                (None, None)
            },
            Some(receiver) => {
                let stat = receiver.borrow();
                if let Some((bytes, elapsed)) = stat.started_at.and_then(|started_at| {
                    stat.uploaded.map(|bytes| {
                        (bytes, started_at.elapsed().unwrap().as_secs())
                    })
                })
                {
                    (Some(bytes), Some(elapsed))
                } else {
                    (None, None)
                }
            }
        };
        PushImageStatusDesc {
            layers: self.layers.clone(),
            current_upload: self.current_upload,
            push_config: self.push_config,
            push_manifest: self.push_manifest,
            done: self.done,
            fault: self.fault.clone(),
            bytes, duration_secs
        }
    }
}

/// The source dataset to be cloned from following by the extraction of layers to create the
/// desired rootfs
#[derive(Clone, Debug)]
pub struct RootFsRecipe {
    chain_id: ChainId,
    /// The dataset to be cloned
    source: Option<ChainId>,
    /// The filesystem layers to be extracted at the cloned root
    digests: Vec<Descriptor>, //    digests: Vec<OciDigest>,

    diff_ids: Vec<OciDigest>,
}

/// Given a dataset, get all the children datasets that have their name conform to the OCI chain id
/// format
fn zfs_list_chain_ids(dataset: impl AsRef<Path>) -> Vec<ChainId> {
    let handle = ZfsHandle::default();
    let mut chain_ids = Vec::new();
    for path in handle.list_direct_children(dataset).unwrap().iter() {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string());
        if let Some(chain_id) = name.and_then(|n| n.parse::<ChainId>().ok()) {
            chain_ids.push(chain_id);
        }
    }
    chain_ids
}

impl RootFsRecipe {
    fn resolve(existed: &[ChainId], diff_id_maps: &[DiffMap]) -> RootFsRecipe {
        if diff_id_maps.is_empty() {
            panic!()
        }

        let algorithm = DigestAlgorithm::Sha256;
        let mut ancestors = Vec::with_capacity(diff_id_maps.len());
        let mut chain_id = oci_util::layer::ChainId::new(&diff_id_maps[0].diff_id);

        // for each layers[..i], calculate the chain_id
        {
            ancestors.push(chain_id.clone());
            for diff_id_map in &diff_id_maps[1..] {
                chain_id.consume_diff_id(algorithm, &diff_id_map.diff_id);
                ancestors.push(chain_id.clone());
            }
        }

        // from the ancestors vector, find the first item from the end of the vector that already
        // already exist in `datasets`
        {
            for (i, id) in ancestors.iter().enumerate().rev() {
                if existed.contains(id) {
                    return RootFsRecipe {
                        chain_id,
                        source: Some(id.clone()),
                        diff_ids: diff_id_maps
                            .iter()
                            .map(|di| di.diff_id.clone())
                            .collect::<Vec<_>>(),
                        digests: if i + 1 == diff_id_maps.len() {
                            Vec::new()
                        } else {
                            diff_id_maps[i + 1..]
                                .iter()
                                .map(|di| di.descriptor.clone())
                                .collect::<Vec<_>>()
                        },
                    };
                }
            }
        }

        RootFsRecipe {
            chain_id,
            source: None,
            digests: diff_id_maps
                .iter()
                .map(|di| di.descriptor.clone())
                .collect::<Vec<_>>(),
            diff_ids: diff_id_maps
                .iter()
                .map(|di| di.diff_id.clone())
                .collect::<Vec<_>>(),
        }
    }

    ///
    /// # Arguments
    /// * `dataset`: The ZFS dataset for images
    /// * `layers_dir`: The directory that contains all the layer diff files
    pub async fn stage_layers_assume_existed(
        &self,
        dataset: impl AsRef<Path>,
        layers_dir: impl AsRef<Path>,
    ) -> Result<(), StageLayerError> {
        let handle = ZfsHandle::default();
        let dataset = dataset.as_ref().to_path_buf();

        // dataset to contain this chain
        let target_dataset = {
            let mut target = dataset.clone();
            target.push(self.chain_id.as_str());
            target
        };

        if let Some(id) = &self.source {
            debug!(id = id.as_str(), "cloning from ancestor dataset");
            let mut source_dataset = dataset;
            source_dataset.push(id.as_str());
            if !handle.exists(&source_dataset) {
                return Err(StageLayerError::SourceDatasetNotFound(id.clone()));
            }
            handle.clone2(&source_dataset, "xc", &target_dataset)?;
        } else {
            debug!("creating new dataset as no ancestors found");
            handle.create2(&target_dataset, false, false)?;
        }

        let layers = self
            .diff_ids
            .iter()
            .fold(String::new(), |a, b| format!("{a},{b}"));
        //        let diff_ids = self.diff_ids.iter().reduce(|a, b| format!("{a},{b}")).unwrap_or_else(String::new);

        // at this point, our datase should exist
        handle.set_prop(&target_dataset, "xc:chain_id", self.chain_id.as_str())?;

        handle.set_prop(&target_dataset, "xc:layers", &layers)?;

        let root = handle
            .mount_point(&target_dataset)?
            .ok_or(StageLayerError::NoMountPoint)?;
        debug!(
            root = root.to_string_lossy().to_string(),
            "begin to extract layers"
        );

        for digest in self.digests.iter() {
            let mut file = layers_dir.as_ref().to_path_buf();
            file.push(digest.digest.as_str());
            let file_path = file.to_string_lossy().to_string();
            debug!(file_path, "extracting");
            _ = tokio::process::Command::new("ocitar")
                .arg("-xf")
                .arg(&file)
                .arg("-C")
                .arg(&root)
                .status()
                .await;
            debug!(file_path, "finished");
        }
        handle.snapshot2(target_dataset, "xc")?;
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum StageLayerError {
    #[error("Cannot expected source dataset to clone from. chain_id: {0}")]
    SourceDatasetNotFound(ChainId),
    #[error("Error on ZFS operation: {0}")]
    ZfsError(ZfsError),
    #[error("dataset has no mountpoint")]
    NoMountPoint,
}

impl From<ZfsError> for StageLayerError {
    fn from(e: ZfsError) -> Self {
        Self::ZfsError(e)
    }
}
