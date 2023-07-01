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

use super::ImageManager;
use super::SharedContext;
use super::DiffMap;
use crate::task::*;

use freebsd::fs::zfs::ZfsHandle;
use oci_util::layer::ChainId;
use oci_util::digest::OciDigest;
use oci_util::distribution::client::*;
use oci_util::image_reference::ImageReference;
use oci_util::models::ImageManifest;
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::sync::watch::Receiver;
use xc::models::jail_image::JailConfig;
use xc::image_store::ImageStore;
use xc::util::get_current_arch;

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

#[derive(Clone, Default, Debug, Deserialize, Serialize)]
pub struct PullLayerStatus {
    pub(super) written: usize,
    pub(super) total: usize,
}

#[derive(Clone, Default, Debug)]
pub struct StageRootStatus {}

#[derive(Clone, Debug)]
pub struct PullImageStatus {
    pub manifest: Option<ImageManifest>,
    pub config: Option<serde_json::Value>,
    pub jail_image: Option<xc::models::jail_image::JailImage>,
}


#[derive(Error, Debug)]
pub enum PullImageError {
    #[error("requested registry not found")]
    RegistryNotFound,
    #[error("request error: {0}")]
    ClientError(ClientError),
    #[error("no usable manifest")]
    NoManifest,
    #[error("no usable config")]
    NoConfig,
    #[error("cannot convert config")]
    ConfigConvertFail
}

pub async fn pull_image(
    this: Arc<RwLock<ImageManager>>,
    reference: ImageReference
) -> Result<Receiver<Task<String, PullImageStatus>>, PullImageError>
{
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
            None => reg.default_registry().ok_or_else(|| PullImageError::RegistryNotFound)?,
            Some(name) => reg
                .get_registry_by_name(&name)
                .unwrap_or_else(|| Registry::new(name, None)),
        }
    };

    let maybe_task = { this.clone().write().await.images.get(&id) };

    if let Some(task_receiver) = maybe_task {
        if !(*task_receiver.borrow()).has_failed() {
            return Ok(task_receiver);
        }
    }

    let (mut emitter, rx) = { this.clone().write().await.images.register(&id) };

    if !emitter.is_completed() {
        let mut session = registry.new_session(image.to_string());
        let manifest = session
            .query_manifest_traced(&tag, |list| {
                list.manifests
                    .iter()
                    .find(|desc| desc.platform.architecture == get_current_arch())
                    .cloned()
            })
            .await
            .map_err(|err| {
                emitter.set_faulted(&format!("failed request manifest: {err:?}"));
                PullImageError::ClientError(err)
            })
            .and_then(|maybe_manifest| {
                emitter.set_faulted("cannot find usable manifest");
                maybe_manifest.ok_or(PullImageError::NoManifest)
            })?;

        _ = emitter.use_try(|state| {
            state.manifest = Some(manifest.clone());
            Ok(())
        });

        let config_descriptor = manifest.config.clone();

        let config: serde_json::Value = session.fetch_blob_as(&config_descriptor.digest).await
            .map_err(|err| {
                emitter.set_faulted(&format!("failed request for config: {err:?}"));
                PullImageError::ClientError(err)
            })
            .and_then(|maybe_config| {
                emitter.set_faulted("cannot find usable config");
                maybe_config.ok_or(PullImageError::NoConfig)
            })?;

        _ = emitter.use_try(|state| {
            state.config = Some(config.clone());
            Ok(())
        });

        let jail_image = JailConfig::from_json(config).ok_or(PullImageError::ConfigConvertFail)?;
        _ = emitter.use_try(|state| {
            state.jail_image = Some(jail_image.clone());
            Ok(())
        });

        if let Some(chain_id) = jail_image.chain_id() {
            let diff_maps = manifest
                .layers
                .iter()
                .zip(jail_image.layers().iter())
                .map(|(descriptor, diff_id)| DiffMap {
                    descriptor: descriptor.clone(),
                    diff_id: diff_id.clone(),
                })
                .collect::<Vec<_>>();
            tokio::spawn(async move {
                let wait_stage_root = {
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
                        _ = image_store
                            .register_and_tag_manifest(
                                &image,
                                &tag,
                                &jail_image
                            )
                            .map_err(|err| {
                                emitter.set_faulted(&format!("{err:?}"));
                                err
                            });
                        emitter.set_completed();
                    }
                    emitter.set_completed();
                }

            });
        } else {
            emitter.set_completed();
        }
    }

    Ok(rx)
}
