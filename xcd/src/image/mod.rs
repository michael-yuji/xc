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
pub mod pull;
pub mod push;

use self::pull::*;
use self::push::*;
use crate::registry::*;
use crate::task::*;

use freebsd::fs::zfs::{ZfsError, ZfsHandle};
use oci_util::digest::{DigestAlgorithm, Hasher, OciDigest};
use oci_util::distribution::client::*;
use oci_util::image_reference::ImageReference;
use oci_util::layer::ChainId;
use oci_util::models::Descriptor;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::watch::Receiver;
use tokio::sync::Mutex;
use tracing::{debug, info};
use xc::image_store::sqlite::SqliteImageStore;
use xc::image_store::{DiffIdMap, ImageRecord, ImageStore, ImageStoreError};
use xc::models::jail_image::JailImage;
use xc::tasks::{DownloadLayerStatus, ImportImageState, ImportImageStatus};

#[derive(Clone)]
pub struct DiffMap {
    pub diff_id: OciDigest,
    pub descriptor: Descriptor,
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

    pub async fn insert_registry(&mut self, id: &str, registry: Registry) {
        self.context
            .registries
            .lock()
            .await
            .insert_registry(id, &registry);
    }

    pub async fn register_and_tag_manifest(
        &self,
        image_reference: &ImageReference,
        manifest: &JailImage,
    ) -> Result<OciDigest, ImageStoreError> {
        self.context
            .image_store
            .lock()
            .await
            .register_and_tag_manifest(image_reference, manifest)
    }

    pub async fn query_manifest(
        &self,
        image_reference: &ImageReference,
    ) -> Result<ImageRecord, ImageStoreError> {
        if image_reference.name == "xc-predefine"
            && image_reference.tag.to_string().as_str() == "empty"
        {
            let manifest = JailImage::default();
            let digest = manifest.digest().to_string();
            let record = ImageRecord {
                image_reference: image_reference.clone(),
                manifest,
                digest,
            };
            Ok(record)
        } else {
            self.context
                .image_store
                .lock()
                .await
                .query_manifest(image_reference)
        }
    }

    pub async fn query_tags(&self, name: &str) -> Result<Vec<ImageRecord>, ImageStoreError> {
        self.context.image_store.lock().await.list_all_tags(name)
    }

    pub async fn list_all_tagged(&self) -> Result<Vec<ImageRecord>, ImageStoreError> {
        self.context.image_store.lock().await.list_all_tagged()
    }

    pub async fn map_diff_id(
        &self,
        diff_id: &OciDigest,
        archive: &OciDigest,
        content_type: &str,
        origin: Option<String>,
    ) -> Result<(), ImageStoreError> {
        self.context
            .image_store
            .lock()
            .await
            .map_diff_id(diff_id, archive, content_type, origin)
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

    pub async fn untag_image(
        &self,
        image_reference: &ImageReference,
    ) -> Result<(), ImageStoreError> {
        self.context.image_store.lock().await.untag(image_reference)
    }

    pub async fn purge(&self) -> Result<(), ImageStoreError> {
        self.context
            .image_store
            .lock()
            .await
            .purge_all_untagged_manifest()?;
        Ok(())
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
                            .map_diff_id(
                                &diff_id,
                                &digest,
                                format,
                                Some(session.repository().to_string()),
                            )
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
}

/// The source dataset to be cloned from following by the extraction of layers to create the
/// desired rootfs
#[derive(Clone, Debug)]
pub struct RootFsRecipe {
    /// expected chain_id
    pub chain_id: ChainId,
    /// The dataset to be cloned
    pub source: Option<ChainId>,
    /// The filesystem layers to be extracted at the cloned root
    pub digests: Vec<Descriptor>, //    digests: Vec<OciDigest>,

    pub diff_ids: Vec<OciDigest>,
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
            // TODO: rollback the dataset first
            handle.snapshot2(&source_dataset, "xc2")?;
            handle.clone2(&source_dataset, "xc2", &target_dataset)?;
            handle.promote(&target_dataset)?;
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
