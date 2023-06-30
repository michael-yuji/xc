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

use crate::digest::OciDigest;

use self::aux::Set;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub const OCI_IMAGE_INDEX: &str = "application/vnd.oci.image.index.v1+json";
pub const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
pub const OCI_ARTIFACT: &str = "application/vnd.oci.artifact.manifest.v1+json";
pub const DOCKER_MANIFESTS: &str = "application/vnd.docker.distribution.manifest.list.v2+json";
pub const DOCKER_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum ManifestVariant {
    Manifest(ImageManifest),
    List(ImageManifestList),
    Artifact(ArtifactManifest),
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Descriptor {
    pub media_type: String,
    pub size: usize,
    pub digest: OciDigest,
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactManifest {
    pub media_type: String,
    pub artifact_type: String,
    pub blobs: Vec<Descriptor>,
    pub subject: Descriptor,
    pub annotations: HashMap<String, String>,
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ImageManifest {
    pub schema_version: u32,
    pub media_type: String,
    pub config: Descriptor,
    pub layers: Vec<Descriptor>,
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Platform {
    pub architecture: String,
    pub os: String,
    #[serde(rename = "os.version")]
    pub os_version: Option<String>,
    #[serde(rename = "os.features", default)]
    pub os_features: Vec<String>,
    /// CPU variant
    pub variant: Option<String>,
    /// CPU features
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ManifestDesc {
    pub media_type: String,
    pub size: usize,
    pub digest: String,
    pub platform: Platform,
    pub artifact_type: Option<String>,
    #[serde(default)]
    pub annotations: HashMap<String, String>,
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ImageManifestList {
    pub schema_version: u32,
    pub media_type: String,
    pub manifests: Vec<ManifestDesc>,
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
pub struct DockerAuthToken {
    // Either token or access_token must exist
    pub token: Option<String>,
    pub access_token: Option<String>,
    pub expires_in: Option<usize>,
    pub issued_at: Option<String>,
}

impl DockerAuthToken {
    pub fn token(&self) -> Option<String> {
        self.token.clone().or(self.access_token.clone())
    }
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
pub struct FreeOciConfig<T> {
    pub architecture: String,
    pub os: String,
    pub config: Option<T>,
    pub rootfs: OciConfigRootFs,
}

impl<T> FreeOciConfig<T> {
    pub fn chain_id(&self) -> crate::layer::ChainId {
        crate::layer::ChainId::calculate_chain_id(
            crate::digest::DigestAlgorithm::Sha256,
            self.rootfs.diff_ids.iter(),
        )
    }
    pub fn layers(&self) -> Vec<OciDigest> {
        self.rootfs.diff_ids.clone()
    }
}

impl<T: Serialize> FreeOciConfig<T> {
    pub fn digest(&self) -> OciDigest {
        let json = serde_json::to_string(&self).unwrap();
        crate::digest::sha256_once(json)
    }
}

pub type AnyOciConfig = FreeOciConfig<Value>;

pub type OciConfig = FreeOciConfig<OciInnerConfig>;

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
pub struct EmptyStruct {}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct OciInnerConfig {
    pub attach_stderr: Option<bool>,
    pub attach_stdin: Option<bool>,
    pub attach_stdout: Option<bool>,
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Option<Vec<String>>,
    pub env: Option<Vec<String>>,
    pub tty: Option<bool>,
    pub working_dir: Option<String>,
    pub volumes: Option<Set<String>>,
    pub exposed_ports: Option<Set<String>>,
    pub user: Option<String>,
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone, Debug)]
pub struct OciConfigRootFs {
    #[serde(rename = "type")]
    pub typ: String,
    pub diff_ids: Vec<OciDigest>,
}

pub mod aux {

    use serde::de::{MapAccess, Visitor};
    use serde::ser::SerializeMap;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::HashSet;
    use std::hash::Hash;
    use std::marker::PhantomData;

    #[derive(Serialize, Deserialize)]
    struct EmptyStruct {}

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Set<T: Hash + Eq>(pub HashSet<T>);

    impl<T: Serialize + Hash + Eq> Default for Set<T> {
        fn default() -> Set<T> {
            Set(HashSet::new())
        }
    }

    impl<T: Serialize + Hash + Eq> Serialize for Set<T> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(self.0.len()))?;
            for key in self.0.iter() {
                map.serialize_entry(&key, &EmptyStruct {})?;
            }
            map.end()
        }
    }

    struct SetVisitor<T> {
        marker: PhantomData<T>,
    }

    impl<T> SetVisitor<T> {
        fn new() -> SetVisitor<T> {
            SetVisitor {
                marker: PhantomData,
            }
        }
    }

    impl<'de, T: Deserialize<'de> + std::hash::Hash + std::cmp::Eq> Visitor<'de> for SetVisitor<T> {
        type Value = Set<T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("object with single entry with key pointing to empty map")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut set = HashSet::with_capacity(map.size_hint().unwrap_or(0));
            while let Some((key, _)) = map.next_entry::<T, EmptyStruct>()? {
                set.insert(key);
            }
            Ok(Set(set))
        }
    }

    impl<'de, T: Deserialize<'de> + std::hash::Hash + std::cmp::Eq> Deserialize<'de> for Set<T> {
        fn deserialize<D>(deserializer: D) -> Result<Set<T>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_map(SetVisitor::new())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::Set;
        use std::collections::HashSet;

        #[test]
        fn test_serialize() {
            let expected = r#"{"hello":{},"world":{}}"#;
            let or = r#"{"world":{},"hello":{}}"#;
            let mut set = HashSet::new();
            set.insert("hello".to_string());
            set.insert("world".to_string());
            let value = super::Set(set);
            let out = serde_json::to_string(&value).unwrap();
            assert!(out == expected || out == or);
        }

        #[test]
        fn test_deserialize() {
            let input = r#"{"hello":{},"world":{}}"#;
            let value: Set<String> = serde_json::from_str(input).unwrap();
            assert!(value.0.contains(&"hello".to_string()));
            assert!(value.0.contains(&"world".to_string()));
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_deserialize_codimd_config() {
        let doc = r#"
{
  "architecture": "amd64",
  "config": {
    "Hostname": "",
    "Domainname": "",
    "User": "",
    "AttachStdin": false,
    "AttachStdout": false,
    "AttachStderr": false,
    "ExposedPorts": {
      "3000/tcp": {}
    },
    "Tty": false,
    "OpenStdin": false,
    "StdinOnce": false,
    "Env": [
      "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
      "HOME=/root",
      "LANGUAGE=en_US.UTF-8",
      "LANG=en_US.UTF-8",
      "TERM=xterm",
      "NODE_ENV=production"
    ],
    "Cmd": null,
    "Image": "sha256:ac8ac73549c02025a924461a3d2322b768197b699912c3ee927092c1d66167c8",
    "Volumes": {
      "/config": {}
    },
    "WorkingDir": "",
    "Entrypoint": [
      "/init"
    ],
    "OnBuild": null,
    "Labels": {
      "build_version": "Linuxserver.io version:- 1.7.0-ls55 Build-date:- 2020-12-24T00:46:41+00:00",
      "maintainer": "chbmb"
    }
  },
  "container": "d6c5564c41d5a4875bc4c9a1595d114a6a929b98bc36d7a285b0e89e0acefc5d",
  "container_config": {
    "Hostname": "d6c5564c41d5",
    "Domainname": "",
    "User": "",
    "AttachStdin": false,
    "AttachStdout": false,
    "AttachStderr": false,
    "ExposedPorts": {
      "3000/tcp": {}
    },
    "Tty": false,
    "OpenStdin": false,
    "StdinOnce": false,
    "Env": [
      "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
      "HOME=/root",
      "LANGUAGE=en_US.UTF-8",
      "LANG=en_US.UTF-8",
      "TERM=xterm",
      "NODE_ENV=production"
    ],
    "Cmd": [
      "/bin/sh",
      "-c",
      " #(nop) ",
      "VOLUME [/config]"
    ],
    "Image": "sha256:ac8ac73549c02025a924461a3d2322b768197b699912c3ee927092c1d66167c8",
    "Volumes": {
      "/config": {}
    },
    "WorkingDir": "",
    "Entrypoint": [
      "/init"
    ],
    "OnBuild": null,
    "Labels": {
      "build_version": "Linuxserver.io version:- 1.7.0-ls55 Build-date:- 2020-12-24T00:46:41+00:00",
      "maintainer": "chbmb"
    }
  },
  "created": "2020-12-24T00:52:27.094774629Z",
  "docker_version": "19.03.14",
  "history": [
    {
      "created": "2020-12-17T15:05:18.886399847Z",
      "created_by": "/bin/sh -c #(nop) COPY dir:6c75c85d1017422948b3b598426f730eddf9d698c857ba989eee2625202435b1 in / "
    },
    {
      "created": "2020-12-17T15:05:19.293721862Z",
      "created_by": "/bin/sh -c #(nop)  ARG BUILD_DATE",
      "empty_layer": true
    },
    {
      "created": "2020-12-17T15:05:19.422701553Z",
      "created_by": "/bin/sh -c #(nop)  ARG VERSION",
      "empty_layer": true
    },
    {
      "created": "2020-12-17T15:05:19.605504218Z",
      "created_by": "/bin/sh -c #(nop)  LABEL build_version=Linuxserver.io version:- d44ccdfe-ls6 Build-date:- 2020-12-17T15:04:31+00:00",
      "empty_layer": true
    },
    {
      "created": "2020-12-17T15:05:19.72374488Z",
      "created_by": "/bin/sh -c #(nop)  LABEL maintainer=TheLamer",
      "empty_layer": true
    },
    {
      "created": "2020-12-17T15:05:19.855782555Z",
      "created_by": "/bin/sh -c #(nop)  ARG OVERLAY_VERSION=v2.1.0.2",
      "empty_layer": true
    },
    {
      "created": "2020-12-17T15:05:19.991813249Z",
      "created_by": "/bin/sh -c #(nop)  ARG OVERLAY_ARCH=amd64",
      "empty_layer": true
    },
    {
      "created": "2020-12-17T15:05:21.204018073Z",
      "created_by": "/bin/sh -c #(nop) ADD 73235ba7e61323f3ed8b78663a44b17e671e1fe0f2b3bacc2611d758717e849c in /tmp/ "
    },
    {
      "created": "2020-12-17T15:05:22.208883092Z",
      "created_by": "|4 BUILD_DATE=2020-12-17T15:04:31+00:00 OVERLAY_ARCH=amd64 OVERLAY_VERSION=v2.1.0.2 VERSION=d44ccdfe-ls6 /bin/sh -c chmod +x /tmp/s6-overlay-${OVERLAY_ARCH}-installer && /tmp/s6-overlay-${OVERLAY_ARCH}-installer / && rm /tmp/s6-overlay-${OVERLAY_ARCH}-installer"
    },
    {
      "created": "2020-12-17T15:05:22.35621222Z",
      "created_by": "/bin/sh -c #(nop)  ARG DEBIAN_FRONTEND=noninteractive",
      "empty_layer": true
    },
    {
      "created": "2020-12-17T15:05:22.484896604Z",
      "created_by": "/bin/sh -c #(nop)  ENV HOME=/root LANGUAGE=en_US.UTF-8 LANG=en_US.UTF-8 TERM=xterm",
      "empty_layer": true
    },
    {
      "created": "2020-12-17T15:05:22.632945087Z",
      "created_by": "/bin/sh -c #(nop) COPY file:a0afddf6c2c99dee6409f51005a22f49cffa569aa0e8ad133ed14c730847fd57 in /etc/apt/ "
    },
    {
      "created": "2020-12-17T15:05:40.615572994Z",
      "created_by": "|5 BUILD_DATE=2020-12-17T15:04:31+00:00 DEBIAN_FRONTEND=noninteractive OVERLAY_ARCH=amd64 OVERLAY_VERSION=v2.1.0.2 VERSION=d44ccdfe-ls6 /bin/sh -c echo \"**** Ripped from Ubuntu Docker Logic ****\" &&  set -xe &&  echo '#!/bin/sh' \t> /usr/sbin/policy-rc.d &&  echo 'exit 101' \t>> /usr/sbin/policy-rc.d &&  chmod +x \t/usr/sbin/policy-rc.d &&  dpkg-divert --local --rename --add /sbin/initctl &&  cp -a \t/usr/sbin/policy-rc.d \t/sbin/initctl &&  sed -i \t's/^exit.*/exit 0/' \t/sbin/initctl &&  echo 'force-unsafe-io' \t> /etc/dpkg/dpkg.cfg.d/docker-apt-speedup &&  echo 'DPkg::Post-Invoke { \"rm -f /var/cache/apt/archives/*.deb /var/cache/apt/archives/partial/*.deb /var/cache/apt/*.bin || true\"; };' \t> /etc/apt/apt.conf.d/docker-clean &&  echo 'APT::Update::Post-Invoke { \"rm -f /var/cache/apt/archives/*.deb /var/cache/apt/archives/partial/*.deb /var/cache/apt/*.bin || true\"; };' \t>> /etc/apt/apt.conf.d/docker-clean &&  echo 'Dir::Cache::pkgcache \"\"; Dir::Cache::srcpkgcache \"\";' \t>> /etc/apt/apt.conf.d/docker-clean &&  echo 'Acquire::Languages \"none\";' \t> /etc/apt/apt.conf.d/docker-no-languages &&  echo 'Acquire::GzipIndexes \"true\"; Acquire::CompressionTypes::Order:: \"gz\";' \t> /etc/apt/apt.conf.d/docker-gzip-indexes &&  echo 'Apt::AutoRemove::SuggestsImportant \"false\";' \t> /etc/apt/apt.conf.d/docker-autoremove-suggests &&  mkdir -p /run/systemd &&  echo 'docker' \t> /run/systemd/container &&  echo \"**** install apt-utils and locales ****\" &&  apt-get update &&  apt-get install -y \tapt-utils \tlocales &&  echo \"**** install packages ****\" &&  apt-get install -y \tcurl \ttzdata &&  echo \"**** generate locale ****\" &&  locale-gen en_US.UTF-8 &&  echo \"**** create abc user and make our folders ****\" &&  useradd -u 911 -U -d /config -s /bin/false abc &&  usermod -G users abc &&  mkdir -p \t/app \t/config \t/defaults &&  mv /usr/bin/with-contenv /usr/bin/with-contenvb &&  echo \"**** cleanup ****\" &&  apt-get clean &&  rm -rf \t/tmp/* \t/var/lib/apt/lists/* \t/var/tmp/*"
    },
    {
      "created": "2020-12-17T15:05:41.010555436Z",
      "created_by": "/bin/sh -c #(nop) COPY dir:643a650c063c923b6bde607ed6d0259227130c48523761aade9a41628ded66d7 in / "
    },
    {
      "created": "2020-12-17T15:05:41.130991583Z",
      "created_by": "/bin/sh -c #(nop)  ENTRYPOINT [\"/init\"]",
      "empty_layer": true
    },
    {
      "created": "2020-12-24T00:47:52.471389116Z",
      "created_by": "/bin/sh -c #(nop)  ARG BUILD_DATE",
      "empty_layer": true
    },
    {
      "created": "2020-12-24T00:47:52.764868617Z",
      "created_by": "/bin/sh -c #(nop)  ARG VERSION",
      "empty_layer": true
    },
    {
      "created": "2020-12-24T00:47:53.069898404Z",
      "created_by": "/bin/sh -c #(nop)  ARG CODIMD_RELEASE",
      "empty_layer": true
    },
    {
      "created": "2020-12-24T00:47:53.351786507Z",
      "created_by": "/bin/sh -c #(nop)  LABEL build_version=Linuxserver.io version:- 1.7.0-ls55 Build-date:- 2020-12-24T00:46:41+00:00",
      "empty_layer": true
    },
    {
      "created": "2020-12-24T00:47:53.631563148Z",
      "created_by": "/bin/sh -c #(nop)  LABEL maintainer=chbmb",
      "empty_layer": true
    },
    {
      "created": "2020-12-24T00:47:53.939238119Z",
      "created_by": "/bin/sh -c #(nop)  ARG DEBIAN_FRONTEND=noninteractive",
      "empty_layer": true
    },
    {
      "created": "2020-12-24T00:47:54.228454385Z",
      "created_by": "/bin/sh -c #(nop)  ENV NODE_ENV=production",
      "empty_layer": true
    },
    {
      "created": "2020-12-24T00:52:20.507993805Z",
      "created_by": "|4 BUILD_DATE=2020-12-24T00:46:41+00:00 CODIMD_RELEASE=1.7.0 DEBIAN_FRONTEND=noninteractive VERSION=1.7.0-ls55 /bin/sh -c echo \"**** install build packages ****\" &&  apt-get update &&  apt-get install -y \tgit \tgnupg \tjq \tlibssl-dev &&  echo \"**** install runtime *****\" &&  curl -s https://deb.nodesource.com/gpgkey/nodesource.gpg.key | apt-key add - &&  echo 'deb https://deb.nodesource.com/node_10.x bionic main' > /etc/apt/sources.list.d/nodesource.list &&  echo \"**** install yarn repository ****\" &&  curl -sS https://dl.yarnpkg.com/debian/pubkey.gpg | apt-key add - &&  echo \"deb https://dl.yarnpkg.com/debian/ stable main\" > /etc/apt/sources.list.d/yarn.list &&  apt-get update &&  apt-get install -y \tfontconfig \tfonts-noto \tnetcat-openbsd \tnodejs \tyarn &&  echo \"**** install codi-md ****\" &&  if [ -z ${CODIMD_RELEASE+x} ]; then \tCODIMD_RELEASE=$(curl -sX GET \"https://api.github.com/repos/hedgedoc/hedgedoc/releases/latest\" \t| awk '/tag_name/{print $4;exit}' FS='[\"\"]');  fi &&  curl -o  /tmp/codimd.tar.gz -L \t\"https://github.com/hedgedoc/hedgedoc/releases/download/${CODIMD_RELEASE}/hedgedoc-${CODIMD_RELEASE}.tar.gz\" &&  mkdir -p \t/opt/codimd &&  tar xf /tmp/codimd.tar.gz -C \t/opt/codimd --strip-components=1 &&  cd /opt/codimd &&  bin/setup &&  echo \"**** cleanup ****\" &&  yarn cache clean &&  apt-get -y purge \tgit \tgnupg \tjq \tlibssl-dev &&  apt-get -y autoremove &&  rm -rf \t/tmp/* \t/var/lib/apt/lists/* \t/var/tmp/*"
    },
    {
      "created": "2020-12-24T00:52:26.475723658Z",
      "created_by": "/bin/sh -c #(nop) COPY dir:49a65a3dd3aef02ca9978b34054d17316b255d13df024b31813097d6264e886f in / "
    },
    {
      "created": "2020-12-24T00:52:26.785758335Z",
      "created_by": "/bin/sh -c #(nop)  EXPOSE 3000",
      "empty_layer": true
    },
    {
      "created": "2020-12-24T00:52:27.094774629Z",
      "created_by": "/bin/sh -c #(nop)  VOLUME [/config]",
      "empty_layer": true
    }
  ],
  "os": "linux",
  "rootfs": {
    "type": "layers",
    "diff_ids": [
      "sha256:69c93d9f9e6b5a1083c2700797b203cd3258eaee33f83688a51edc715e21303e",
      "sha256:9c8b9e8235ecf05ff1b3c8282f080ed944f93cb0779d855394e72ad1dd7a2f9e",
      "sha256:1c531bc3aa65fa8affc04e138087496e837b16ef73e87ca4da70f3ddac3f1dc0",
      "sha256:ed40f2cd68c1f93a1b520ce6822c98c1166b837642bcd30f3ca79ab5e6d21691",
      "sha256:7e86d74d32ac9792b26bbe957155022302c4640309b36e1c525412f8db3a3179",
      "sha256:535f21c984e40e6989753dfe8da5ab9ffbcac7d870ddd80a36fb30727692a036",
      "sha256:58d0a797ab3b6290ceed89fa3329bd9e3d006cd83cab256d73ac62c128b6f57e",
      "sha256:2cd43756113896787f5dd5131dd2b3fc1800cedeedfa67243e2dd2bb0c14a19c"
    ]
  }
}
        "#;

        serde_json::from_str::<super::OciConfig>(doc).expect("cannot decode oci config");
    }
}
