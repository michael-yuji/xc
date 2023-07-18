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
use crate::context::ServerContext;
use crate::image::pull::PullImageError;
use crate::image::push::{PushImageError, PushImageStatusDesc};
use freebsd::event::EventFdNotify;
use freebsd::libc::{EINVAL, EIO};
use ipc::packet::codec::{Fd, FromPacket, List, Maybe};
use ipc::proto::{enoent, ipc_err, GenericResult};
use ipc::service::{ConnectionContext, Service};
use ipc_macro::{ipc_method, FromPacket};
use oci_util::digest::OciDigest;
use oci_util::distribution::client::{BasicAuth, Registry};
use oci_util::image_reference::ImageReference;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Seek;
use std::net::IpAddr;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::*;
use xc::container::request::{MountReq, NetworkAllocRequest};
use xc::models::exec::{Jexec, StdioMode};
use xc::models::jail_image::JailConfig;
use xc::models::network::{DnsSetting, IpAssign, PortRedirection};
use xc::res::network::Network;
use xc::util::{gen_id, CompressionFormat, CompressionFormatExt};

#[derive(FromPacket, Debug)]
pub struct CreateChannelRequest {
    pub socket_path: String,
}

#[derive(FromPacket, Debug)]
pub struct CreateChannelResponse {}

pub trait MethodError {
    fn errno(&self) -> u8;
    fn serialize(&self) -> serde_json::Value;
}

#[derive(Clone, Debug)]
pub struct Variables {
    linked_container_id: Option<String>,
}

pub fn do_xc_request<T: FromPacket>(
    stream: &mut UnixStream,
    method: &str,
    request: impl FromPacket,
) -> Result<GenericResult<T>, ipc::transport::ChannelError<ipc::proto::IpcError>> {
    use ipc::packet::codec::json::JsonPacket;
    use ipc::proto::IpcError;
    use ipc::transport::PacketTransport;

    let packet = request
        .to_packet_failable(|dual| {
            let value = serde_json::to_value(dual)?;
            let req = ipc::proto::Request {
                method: method.to_string(),
                value,
            };
            serde_json::to_vec(&req)
        })
        .map_err(IpcError::Serde)?;

    stream
        .send_packet(&packet)
        .map_err(|e| e.map(IpcError::Io))?;

    let packet = stream.recv_packet().map_err(|e| e.map(IpcError::Io))?;
    let json_packet = JsonPacket::new(packet).map_err(IpcError::Serde)?;
    let response: ipc::packet::TypedPacket<ipc::proto::Response> = json_packet
        .map_failable(|value| serde_json::from_value(value.clone()))
        .map_err(IpcError::Serde)?;

    if response.data.errno == 0 {
        let t = T::from_packet_failable(response, |inner| {
            serde_json::from_value(inner.value.clone())
        })
        .map_err(IpcError::Serde)?;

        Ok(Ok(t))
    } else {
        let err = response.data.to_err_typed()?;
        Ok(Err(err))
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct EmptyResponse {}

#[ipc_method(method = "create_channel")]
async fn create_channel(
    context: Arc<RwLock<ServerContext>>,
    load_content: &mut ConnectionContext<Variables>,
    request: CreateChannelRequest,
) -> GenericResult<CreateChannelResponse> {
    let path = std::path::Path::new(&request.socket_path);
    ServerContext::create_channel(context, path).unwrap();
    Ok(CreateChannelResponse {})
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InfoRequest {}

#[derive(Serialize, Deserialize, Debug)]
pub struct InfoResponse {
    pub config: xc::config::XcConfig,
}

#[ipc_method(method = "info")]
async fn info(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: InfoRequest,
) -> GenericResult<InfoResponse> {
    let config = context.read().await.config();
    Ok(InfoResponse { config })
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PullImageRequest {
    pub image_reference: ImageReference,
    pub rename_reference: Option<ImageReference>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PullImageResponse {
    pub existed: bool,
}

#[ipc_method(method = "pull_image")]
async fn pull_image(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: PullImageRequest,
) -> GenericResult<PullImageResponse> {
    let result = context
        .write()
        .await
        .pull_image(request.image_reference, request.rename_reference)
        .await;

    match result {
        Err(PullImageError::NoConfig) => enoent("cannot find oci config from registry"),
        Err(PullImageError::NoManifest) => enoent("cannot find usable manifest from registry"),
        Err(PullImageError::RegistryNotFound) => enoent("requested registry not found"),
        Err(PullImageError::ConfigConvertFail) => enoent("failure on config conversion"),
        Err(PullImageError::ClientError(response)) => {
            error!("pull image result in error response: {response:?}");
            ipc_err(EINVAL, &format!("client error: {response:?}"))
        }
        Ok(_) => Ok(PullImageResponse { existed: false }),
    }
}

#[derive(FromPacket)]
pub struct CopyFile {
    pub source: Fd,
    pub destination: String,
}

#[derive(FromPacket)]
pub struct InstantiateRequest {
    pub image_reference: ImageReference,
    pub alt_root: Option<String>,
    pub envs: HashMap<String, String>,
    pub vnet: bool,
    pub ips: Vec<IpAssign>,
    pub ipreq: Vec<NetworkAllocRequest>,
    pub mount_req: Vec<MountReq>,
    pub copies: List<CopyFile>,
    pub entry_point: String,
    pub entry_point_args: Vec<String>,
    pub hostname: Option<String>,
    pub main_norun: bool,
    pub init_norun: bool,
    pub deinit_norun: bool,
    pub persist: bool,
    pub no_clean: bool,
    pub name: Option<String>,
    pub dns: DnsSetting,

    pub extra_layers: List<Fd>,

    pub main_started_notify: Maybe<Fd>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InstantiateResponse {
    pub id: String,
}

#[ipc_method(method = "instantiate")]
async fn instantiate(
    context: Arc<RwLock<ServerContext>>,
    load_context: &mut ConnectionContext<Variables>,
    request: InstantiateRequest,
) -> GenericResult<InstantiateResponse> {
    //    let id = Uuid::new_v4().to_string();
    let id = gen_id();

    if request
        .name
        .as_ref()
        .and_then(|n| n.parse::<isize>().ok())
        .is_some()
    {
        return ipc_err(EINVAL, "container name cannot be integer literal");
    }

    let row = {
        let ctx = context.read().await;
        let dlctx = ctx.image_manager.read().await;
        let image_name = if let Some(hostname) = &request.image_reference.hostname {
            format!("{hostname}/{}", request.image_reference.name)
        } else {
            request.image_reference.name.to_string()
        };
        let image_tag = request.image_reference.tag.to_string();
        dlctx.query_manifest(&image_name, &image_tag).await
    };

    match row {
        Err(_) => enoent("image not found"),
        Ok(image_row) => {
            let credential = Credential::from_conn_ctx(local_context);
            let instantiate_result =
                ServerContext::instantiate(context, &id, &image_row.manifest, request, credential)
                    .await;
            if let Err(error) = instantiate_result {
                tracing::error!("instantiate error: {error:#?}");
                if let Some(err) = error.downcast_ref::<xc::container::error::PreconditionFailure>()
                {
                    ipc_err(err.errno(), &err.error_message())
                } else {
                    enoent(error.to_string().as_str())
                }
            } else {
                Ok(InstantiateResponse { id })
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UploadStat {
    pub image_reference: ImageReference,
    pub remote_reference: ImageReference,
}

#[ipc_method(method = "upload_stat")]
async fn upload_stat(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: UploadStat,
) -> GenericResult<PushImageStatusDesc> {
    //    let id = request.image_reference.to_string();
    let id = format!("{}->{}", request.image_reference, request.remote_reference);
    let state = context
        .read()
        .await
        .image_manager
        .write()
        .await
        .get_upload_state(&id);
    Ok(state)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DownloadStat {
    pub image_reference: ImageReference,
}

#[ipc_method(method = "download_stat")]
async fn download_stat(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: DownloadStat,
) -> GenericResult<xc::tasks::ImportImageStatus> {
    let id = request.image_reference.to_string();
    let state = context
        .read()
        .await
        .image_manager
        .write()
        .await
        .get_download_state(&id);
    Ok(state)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ListManifestsRequest {}

#[derive(Serialize, Deserialize, Debug)]
pub struct ListManifestsResponse2 {
    pub manifests: Vec<xc::image_store::ImageRecord>,
}

#[ipc_method(method = "list_all_images")]
async fn list_all_images(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: ListManifestsRequest,
) -> GenericResult<ListManifestsResponse2> {
    let manifests = context.read().await.list_all_images().await.unwrap();
    Ok(ListManifestsResponse2 { manifests })
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DescribeImageRequest {
    pub image_name: String,
    pub tag: String,
}
#[derive(Serialize, Deserialize, Debug)]
pub struct DescribeImageResponse {
    pub image_name: String,
    pub tag: String,
    pub chain_id: String,
    pub digest: String,
    pub jail_image: xc::models::jail_image::JailImage,
    pub dataset_id: String,
}

#[derive(Serialize, Deserialize, thiserror::Error, Debug)]
pub enum DescribeImageError {
    #[error("Image not found")]
    ImageReferenceNotFound,
}

#[ipc_method(method = "describe_image")]
async fn describe_image(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: DescribeImageRequest,
) -> Result<DescribeImageResponse, ipc::proto::ErrResponse<DescribeImageError>> {
    let image = context
        .read()
        .await
        .resolve_image(&request.image_name, &request.tag)
        .await
        .unwrap()
        .ok_or(DescribeImageError::ImageReferenceNotFound)
        .map_err(|value| ipc::proto::ErrResponse { errno: 1, value })?;

    let chain_id = image.manifest.chain_id().unwrap().to_string();

    Ok(DescribeImageResponse {
        image_name: image.name,
        dataset_id: chain_id.clone(),
        tag: image.tag,
        chain_id,
        digest: image.digest,
        jail_image: image.manifest,
    })
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DescribeImagesRequest {
    pub image_name: String,
}
#[ipc_method(method = "describe_images")]
async fn describe_images(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: DescribeImagesRequest,
) -> GenericResult<Vec<DescribeImageResponse>> {
    let image_rows = context
        .read()
        .await
        .list_images(&request.image_name)
        .await
        .unwrap();
    let mut rows = Vec::new();

    for image_row in image_rows.into_iter() {
        let chain_id = image_row
            .manifest
            .chain_id()
            .map(|a| a.to_string())
            .unwrap_or_else(String::new);

        let chain_id2 = chain_id.clone().to_string();

        rows.push(DescribeImageResponse {
            image_name: image_row.name,
            tag: image_row.tag,
            chain_id,
            digest: image_row.digest,
            jail_image: image_row.manifest,
            dataset_id: chain_id2,
        });
    }

    Ok(rows)
}

#[ipc_method(method = "remove_image")]
async fn remove_image(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: ImageReference,
) -> GenericResult<()> {
    // XXX: Handle @<digest> tag
    context
        .read()
        .await
        .image_manager
        .read()
        .await
        .untag_image(&request.name, &request.tag.to_string())
        .await
        .unwrap();
    Ok(())
}

#[ipc_method(method = "purge")]
async fn purge(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: (),
) -> GenericResult<()> {
    context.read().await.purge_images().await.unwrap();
    Ok(())
}
#[derive(Serialize, Deserialize, Debug)]
pub struct ListNetworkRequest {}

#[derive(Serialize, Deserialize, Debug)]
pub struct ListNetworkResponse {
    pub network_info: Vec<crate::network_manager::NetworkInfo>,
}

#[ipc_method(method = "list_networks")]
async fn list_networks(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: ListNetworkRequest,
) -> GenericResult<ListNetworkResponse> {
    let context = context.read().await;
    let network_manager = context.network_manager.clone();
    let network_manager = network_manager.lock().await;
    match network_manager.get_network_info() {
        Ok(network_info) => Ok(ListNetworkResponse { network_info }),
        Err(e) => ipc_err(EIO, e.to_string().as_str()),
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CreateNetworkRequest {
    pub name: String,
    //    pub ext_if: Option<String>,
    pub alias_iface: String,
    pub bridge_iface: String,
    pub subnet: ipcidr::IpCidr,
    pub start_addr: Option<IpAddr>,
    pub end_addr: Option<IpAddr>,
    pub default_router: Option<IpAddr>,
}

#[ipc_method(method = "create_network")]
async fn create_network(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: CreateNetworkRequest,
) -> GenericResult<()> {
    let context = context.write().await;
    let config = context.config();
    let existing_ifaces = freebsd::net::ifconfig::interfaces().unwrap();
    if config.networks.contains_key(&request.name) {
        ipc_err(EINVAL, "Network with such name already exists")
    } else {
        let nm = context.network_manager.clone();
        let nm = nm.lock().await;

        if !existing_ifaces.contains(&request.alias_iface) {
            return enoent(format!("interface {} not found", request.alias_iface).as_str());
        } else if !existing_ifaces.contains(&request.bridge_iface) {
            return enoent(format!("interface {} not found", request.bridge_iface).as_str());
        }

        let network = Network {
            ext_if: None,
            alias_iface: request.alias_iface.to_string(),
            bridge_iface: request.bridge_iface.to_string(),
            subnet: request.subnet,
            start_addr: request.start_addr,
            end_addr: request.end_addr,
            default_router: request.default_router,
        };

        match nm.create_network(&request.name, &network) {
            Ok(_) => {
                info!("created new network: {}", request.name);
                context.config_manager.modify_config(|config| {
                    config.networks.insert(request.name.to_string(), network);
                });
                Ok(())
            }
            Err(e) => {
                warn!("failed to create network due to sqlite error: {e}");
                ipc_err(EIO, "failed to create such network due to sqlite error")
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ListContainersRequest {}

#[derive(Serialize, Deserialize, Debug)]
pub struct ListContainersResponse {}

#[ipc_method(method = "list_containers")]
async fn list_containers(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: (),
) -> GenericResult<Vec<xc::container::ContainerManifest>> {
    Ok(context.read().await.list_containers().await)
}
#[derive(Serialize, Deserialize, Debug)]
pub struct ShowContainerRequest {
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ShowContainerResponse {
    pub running_container: xc::container::ContainerManifest,
}

#[ipc_method(method = "show_container")]
async fn show_container(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: ShowContainerRequest,
) -> GenericResult<ShowContainerResponse> {
    if let Some(container) = context
        .read()
        .await
        .resolve_container_by_name(&request.id)
        .await
    {
        Ok(ShowContainerResponse {
            running_container: container,
        })
    } else {
        enoent("container with such identifier not found")
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct KillContainerRequest {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct KillContainerResponse {}

#[ipc_method(method = "kill_container")]
async fn kill_container(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: KillContainerRequest,
) -> GenericResult<KillContainerResponse> {
    let mut context = context.write().await;
    if let Some(id) = context.alias_map.get(&request.name).cloned() {
        _ = context.terminate(&id).await;
        Ok(KillContainerResponse {})
    } else {
        enoent(format!("no such container: {}", request.name).as_str())
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DoRdr {
    pub name: String,
    pub redirection: PortRedirection,
}
#[ipc_method(method = "rdr")]
async fn rdr_container(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: DoRdr,
) -> GenericResult<DoRdr> {
    let name = request.name.clone();
    let redirection = request.redirection.clone();
    _ = context.write().await.do_rdr(&name, &redirection).await;
    Ok(request)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ContainerRdrList {
    pub name: String,
}

#[ipc_method(method = "list_site_rdr")]
async fn list_site_rdr(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: ContainerRdrList,
) -> GenericResult<Vec<PortRedirection>> {
    let context = context.read().await;
    if let Some(id) = context.alias_map.get(&request.name) {
        Ok(context.port_forward_table.all_rules_with_id(id))
    } else {
        enoent(format!("no such container: {}", request.name).as_str())
    }
}

#[derive(FromPacket)]
pub struct FdImport {
    pub fd: Fd,
    pub config: JailConfig,
    pub image_reference: ImageReference,
}

// XXX: Handle loads of error here
#[ipc_method(method = "fd_import")]
async fn fd_import(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: FdImport,
) -> GenericResult<oci_util::digest::OciDigest> {
    let config = { context.read().await.config() };
    let image_dataset = config.image_dataset.to_string();
    let layers_dir = config.layers_dir.to_string();
    let zfs = freebsd::fs::zfs::ZfsHandle::default();
    let temp = gen_id();
    let tempdataset = format!("{image_dataset}/{temp}");
    let tempfile_path = format!("{layers_dir}/{temp}");
    let tempfile = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .open(&tempfile_path)
        .unwrap();

    zfs.create2(&tempdataset, false, false).unwrap();

    let mountpoint = zfs.mount_point(&tempdataset).unwrap().unwrap();

    let source_fd = request.fd.as_raw_fd();
    let dest_fd = tempfile.as_raw_fd();
    let mut file = unsafe { std::fs::File::from_raw_fd(source_fd) };
    let file_len = file.metadata().unwrap().len() as usize;

    let content_type = match file.compression_format().expect("cannot read magic") {
        CompressionFormat::Gzip => "gzip",
        CompressionFormat::Zstd => "zstd",
        CompressionFormat::Other => "plain",
    };

    info!("import: content_type is {content_type}");

    unsafe {
        freebsd::nix::libc::copy_file_range(
            source_fd,
            std::ptr::null_mut(),
            dest_fd,
            std::ptr::null_mut(),
            file_len,
            0,
        );
    }

    info!("copy_file_range done");
    drop(tempfile);

    file.rewind();

    let ocitar_output = std::process::Command::new("ocitar")
        .arg("-xf-")
        .arg("--print-input-digest")
        .arg("-C")
        .arg(mountpoint)
        .stdin(file)
        .output()
        .unwrap();

    let output_lines = std::str::from_utf8(&ocitar_output.stdout)
        .unwrap()
        .lines()
        .collect::<Vec<_>>();
    let diff_id = OciDigest::new_unchecked(output_lines[0].trim());
    let archive_digest = OciDigest::new_unchecked(output_lines[1].trim());

    info!("diff_id: {diff_id}");
    info!("archive_digest: {archive_digest}");

    _ = std::fs::rename(tempfile_path, format!("{layers_dir}/{archive_digest}"));

    let dataset = format!("{image_dataset}/{diff_id}");
    zfs.rename(&tempdataset, &dataset).unwrap();
    zfs.snapshot2(dataset, "xc").unwrap();

    {
        context
            .write()
            .await
            .import_fat_tar(
                &diff_id,
                &archive_digest,
                content_type,
                &request.config,
                &request.image_reference.name,
                request.image_reference.tag.as_str(),
            )
            .await
    }
    Ok(diff_id)
}

#[derive(FromPacket)]
pub struct CommitRequest {
    pub container_name: String,
    pub name: String,
    pub tag: String,
    pub alt_out: Maybe<Fd>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CommitResponse {
    pub commit_id: String,
}

#[ipc_method(method = "commit")]
async fn commit_container(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: CommitRequest,
) -> GenericResult<CommitResponse> {
    let mut ctx = context.write().await;
    let result = if let Maybe::Some(fd) = request.alt_out {
        ctx.do_commit_file(&request.container_name, fd.0).await
    } else {
        ctx.do_commit(&request.container_name, &request.name, &request.tag)
            .await
    }
    .map(|s| s.to_string());
    match result {
        Ok(commit_id) => {
            let response = CommitResponse { commit_id };
            Ok(response)
        }
        Err(err) => {
            error!("{err:#?}");
            enoent(&format!("{err:#?}"))
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SetConfigRequest {
    pub name: String,
    pub tag: String,
    pub config: xc::models::jail_image::JailConfig,
}

#[ipc_method(method = "replace_meta")]
async fn replace_meta(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: SetConfigRequest,
) -> GenericResult<xc::models::jail_image::JailImage> {
    let ctx = context.read().await;
    let dlctx = ctx.image_manager.read().await;
    let record = dlctx
        .query_manifest(&request.name, &request.tag)
        .await
        .unwrap();
    let mut manifest = record.manifest;
    manifest.set_config(&request.config);
    dlctx
        .register_and_tag_manifest(&request.name, &request.tag, &manifest)
        .await
        .unwrap();
    Ok(manifest)
}

#[derive(FromPacket)]
pub struct LinkContainerRequest {
    pub name: String,
    pub fd: ipc::packet::codec::Fd,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LinkContainerResponse {}

#[ipc_method(method = "link")]
async fn link_container(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: LinkContainerRequest,
) -> GenericResult<LinkContainerResponse> {
    let context = context.write().await;
    if let Some(site) = context.get_site(&request.name) {
        local_context.udata = Some(Variables {
            linked_container_id: Some(request.name.clone()),
        });
        site.write().await.link_fd(request.fd.0);
        Ok(LinkContainerResponse {})
    } else {
        enoent("No such container")
    }
}

#[derive(Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub server: String,
    pub insecure: bool,
}

#[ipc_method(method = "login")]
async fn login_registry(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: LoginRequest,
) -> GenericResult<()> {
    let scheme = if request.insecure { "http" } else { "https" };
    let registry = Registry::new(
        format!("{scheme}://{}", request.server),
        Some(BasicAuth::new(request.username, request.password)),
    );
    // XXX: should have find some ways to verify the tokens
    context
        .write()
        .await
        .image_manager
        .write()
        .await
        .insert_registry(&request.server, registry)
        .await;
    Ok(())
}

#[derive(FromPacket)]
pub struct ExecCommandRequest {
    pub name: String,
    pub arg0: String,
    pub args: Vec<String>,
    pub envs: HashMap<String, String>,
    pub stdin: Maybe<Fd>,
    pub stdout: Maybe<Fd>,
    pub stderr: Maybe<Fd>,
    pub uid: u32,
    pub notify: Maybe<Fd>,
    pub use_tty: bool,
}

#[ipc_method(method = "exec")]
async fn exec(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: ExecCommandRequest,
) -> GenericResult<xc::container::process::SpawnInfo> {
    let jexec = Jexec {
        arg0: request.arg0,
        args: request.args,
        envs: request.envs,
        uid: request.uid,
        output_mode: if request.use_tty {
            StdioMode::Terminal
        } else {
            StdioMode::Forward {
                stdin: request.stdin.to_option().map(|fd| fd.0),
                stdout: request.stdout.to_option().map(|fd| fd.0),
                stderr: request.stderr.to_option().map(|fd| fd.0),
            }
        },
        notify: request.notify.to_option().map(|fd| fd.0),
        work_dir: None,
    };
    if let Some(arc_site) = context.write().await.get_site(&request.name) {
        let mut site = arc_site.write().await;
        site.exec(jexec)
    } else {
        enoent("container not found")
    }
}

#[derive(Serialize, Deserialize)]
pub struct NetgroupCommit {
    pub netgroup_name: String,
}

#[ipc_method(method = "commit_netgroup")]
async fn commit_netgroup(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: NetgroupCommit,
) -> GenericResult<()> {
    context
        .write()
        .await
        .update_hosts(&request.netgroup_name)
        .await;
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub struct NetgroupAddContainerRequest {
    pub netgroup_name: String,
    pub container_name: String,
    pub auto_create_netgroup: bool,
    pub commit_immediately: bool,
}

#[ipc_method(method = "add-netgroup")]
async fn add_container_to_netgroup(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: NetgroupAddContainerRequest,
) -> GenericResult<()> {
    let ng_name = request.netgroup_name.to_string();
    let mut context = context.write().await;

    if let Some(container_id) = context.alias_map.get(&request.container_name) {
        let cid = container_id.to_string();

        let js = if let Some(jails) = context.ng2jails.get_mut(&request.netgroup_name) {
            jails.push(cid.to_string());
            jails.clone()
        } else if request.auto_create_netgroup {
            context
                .ng2jails
                .insert(ng_name.to_string(), vec![cid.to_string()]);
            vec![cid.to_string()]
        } else {
            return enoent("netgroup not found");
        };

        if let Some(ngs) = context.jail2ngs.get_mut(&cid) {
            ngs.push(ng_name.to_string());
        } else {
            context.jail2ngs.insert(cid, vec![ng_name.to_string()]);
        };

        if request.commit_immediately {
            context.update_hosts(&ng_name).await;
        }
        Ok(())
    } else {
        enoent("container not found")
    }
}

#[derive(FromPacket)]
pub struct RunMainRequest {
    pub name: String,
    pub notify: Maybe<Fd>,
}

#[derive(FromPacket)]
pub struct RunMainResponse {
    pub id: String,
}

/// XXX: Temporary
#[ipc_method(method = "run_main")]
async fn run_main(
    context: Arc<RwLock<ServerContext>>,
    local_context: &mut ConnectionContext<Variables>,
    request: RunMainRequest,
) -> GenericResult<RunMainResponse> {
    if let Some(arc_site) = context.write().await.get_site(&request.name) {
        let mut site = arc_site.write().await;
        let notify = request
            .notify
            .to_option()
            .map(|fd| EventFdNotify::from_fd(fd.0));
        site.run_main(notify);
        Ok(RunMainResponse { id: site.id() })
    } else {
        enoent("container not found")
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PushImageRequest {
    pub image_reference: ImageReference,
    pub remote_reference: ImageReference,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PushImageResponse {}

#[ipc_method(method = "push_image")]
async fn push_image(
    context: Arc<RwLock<ServerContext>>,
    load_context: &mut ConnectionContext<Variables>,
    request: PushImageRequest,
) -> Result<PushImageResponse, ipc::proto::ErrResponse<PushImageError>> {
    let ctx = context.read().await;
    ctx.push_image(request.image_reference, request.remote_reference)
        .await
        .map(|_| PushImageResponse {})
        .map_err(|err| ipc::proto::ErrResponse {
            value: err,
            errno: 1,
        })
}

#[allow(non_upper_case_globals)]
const on_channel_closed: OnChannelClosed = OnChannelClosed {};

struct OnChannelClosed {}

#[async_trait::async_trait]
impl ipc::service::StreamDelegate<RwLock<ServerContext>, Variables> for OnChannelClosed {
    async fn on_event(
        &self,
        context: Arc<RwLock<ServerContext>>,
        conn_ctx: &mut ConnectionContext<Variables>,
        event: ipc::service::StreamEvent,
    ) -> anyhow::Result<()> {
        if let ipc::service::StreamEvent::ConnectionClosed = event {
            if let Some(container_id) = conn_ctx.udata.clone().and_then(|v| v.linked_container_id) {
                let mut context = context.write().await;
                if let Some(id) = context.alias_map.get(&container_id).cloned() {
                    _ = context.terminate(&id).await;
                }
            }
        }
        Ok(())
    }
}

pub(crate) async fn register_to_service(
    service: &mut Service<tokio::sync::RwLock<ServerContext>, Variables>,
) {
    service.register_event_delegate(on_channel_closed).await;
    service.register(commit_netgroup).await;
    service.register(add_container_to_netgroup).await;
    service.register(purge).await;
    service.register(remove_image).await;
    service.register(create_channel).await;
    service.register(exec).await;
    service.register(fd_import).await;
    service.register(info).await;
    service.register(link_container).await;
    service.register(list_all_images).await;
    service.register(pull_image).await;
    service.register(describe_image).await;
    service.register(describe_images).await;
    service.register(instantiate).await;
    service.register(create_network).await;
    service.register(list_networks).await;
    service.register(show_container).await;
    service.register(kill_container).await;
    service.register(list_containers).await;
    service.register(login_registry).await;
    service.register(commit_container).await;
    service.register(download_stat).await;
    service.register(upload_stat).await;
    service.register(rdr_container).await;
    service.register(replace_meta).await;
    service.register(run_main).await;
    service.register(push_image).await;
}
