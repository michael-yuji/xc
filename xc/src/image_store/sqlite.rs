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
use super::{DiffIdMap, ImageRecord, ImageStore, ImageStoreError};
use crate::models::jail_image::JailImage;
use oci_util::digest::OciDigest;
use oci_util::image_reference::{ImageReference, ImageTag};
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;
use std::str::FromStr;

impl From<rusqlite::Error> for ImageStoreError {
    fn from(value: rusqlite::Error) -> ImageStoreError {
        ImageStoreError::EngineError(anyhow::Error::new(value))
    }
}

impl From<serde_json::Error> for ImageStoreError {
    fn from(value: serde_json::Error) -> ImageStoreError {
        ImageStoreError::EngineError(anyhow::Error::new(value))
    }
}

impl From<std::io::Error> for ImageStoreError {
    fn from(value: std::io::Error) -> ImageStoreError {
        ImageStoreError::EngineError(anyhow::Error::new(value))
    }
}

#[derive(Debug)]
pub struct SqliteImageStore {
    db: Connection,
}

impl SqliteImageStore {
    pub fn open_in_memory() -> SqliteImageStore {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        SqliteImageStore { db }
    }

    pub fn open_file(path: impl AsRef<std::path::Path>) -> SqliteImageStore {
        let db = rusqlite::Connection::open(path.as_ref()).unwrap();
        SqliteImageStore { db }
    }

    pub fn create_tables(&self) -> Result<(), rusqlite::Error> {
        self.db.execute_batch(
            "
            create table if not exists diff_id_map (
                diff_id text not null,
                digest text not null,
                compress_alg text not null,
                origin text,
                primary key (diff_id, digest)
            );

            create table if not exists image_manifests (
                manifest text not null,
                digest text not null primary key
            );

            create table if not exists image_manifest_refs (
                hostname text not null,
                name text not null,
                digest text not null,
                primary key (hostname, name, digest),
                foreign key (digest)
                    references image_manifests(digest)
                    on delete cascade
            );

            create table if not exists image_manifest_tags (
                hostname text,
                name text not null,
                tag text not null,
                digest text not null,
                primary key (hostname, name, tag),
                foreign key (digest)
                    references image_manifests(digest)
                    on delete cascade
            );
            ",
        )?;

        if self
            .db
            .execute("alter table diff_id_map add column origin text;", [])
            .is_ok()
        {
            tracing::info!("UPDATED DIFF_ID_MAP DATABASE SCHEME");
        };

        Ok(())
    }
}

impl ImageStore for SqliteImageStore {
    fn purge_all_untagged_manifest(&self) -> Result<(), ImageStoreError> {
        // TODO: deal with digest reference
        self.db.execute(
            "
            delete from image_manifests where digest not in (select digest from image_manifest_tags)
            ",
            [],
        )?;
        Ok(())
    }

    fn query_diff_id(&self, digest: &OciDigest) -> Result<Option<DiffIdMap>, ImageStoreError> {
        let mut stmt = self.db.prepare_cached(
            "
            select
                diff_id,
                digest,
                compress_alg,
                origin
            from diff_id_map where digest=?",
        )?;
        let imap: Option<DiffIdMap> = stmt
            .query_row([digest.as_str()], |row| {
                let s_diff_id: String = row.get(0)?;
                let s_digest: String = row.get(1)?;
                Ok(DiffIdMap {
                    diff_id: OciDigest::from_str(&s_diff_id).unwrap(),
                    archive_digest: OciDigest::from_str(&s_digest).unwrap(),
                    algorithm: row.get(2)?,
                    origin: row.get(3)?,
                })
            })
            .optional()?;
        Ok(imap)
    }

    fn query_archives(&self, diff_id: &OciDigest) -> Result<Vec<DiffIdMap>, ImageStoreError> {
        let mut stmt = self.db.prepare_cached(
            "select diff_id, digest, compress_alg, origin from diff_id_map where diff_id=?",
        )?;
        let mut rows = stmt.query([diff_id.as_str()])?;
        let mut records = Vec::new();
        while let Ok(Some(row)) = rows.next() {
            let s_diff_id: String = row.get(0)?;
            let s_digest: String = row.get(1)?;

            records.push(DiffIdMap {
                diff_id: OciDigest::from_str(&s_diff_id).unwrap(),
                archive_digest: OciDigest::from_str(&s_digest).unwrap(),
                algorithm: row.get(2)?,
                origin: row.get(3)?,
            });
        }
        Ok(records)
    }

    fn map_diff_id(
        &self,
        diff_id: &OciDigest,
        archive: &OciDigest,
        content_type: &str,
        origin: Option<String>,
    ) -> Result<(), ImageStoreError> {
        let mut stmt = self.db.prepare_cached(
            "
            insert into diff_id_map (diff_id, digest, compress_alg)
            values (?, ?, ?), (?, ?, ?)
                on conflict (diff_id, digest) do nothing
            ",
        )?;
        stmt.execute([
            diff_id.as_str(),
            diff_id.as_str(),
            "plain",
            diff_id.as_str(),
            archive.as_str(),
            content_type,
        ])?;

        if let Some(origin) = origin {
            let mut stmt = self.db.prepare_cached(
                "update diff_id_map set origin=? where (diff_id, digest) = (?, ?)",
            )?;
            stmt.execute([origin.as_str(), diff_id.as_str(), archive.as_str()])?;
        }
        Ok(())
    }

    fn delete_manifest(&self, digest: &OciDigest) -> Result<(), ImageStoreError> {
        let db = &self.db;
        let mut stmt = db.prepare_cached("delete from image_manifests where digest=?")?;
        stmt.execute([digest.as_str()])?;
        let mut stmt2 = db.prepare_cached("delete from image_manifest_tags where digest=?")?;
        stmt2.execute([digest.as_str()])?;
        let mut stmt3 = db.prepare_cached("delete from image_manifest_refs where digest=?")?;
        stmt3.execute([digest.as_str()])?;
        Ok(())
    }

    fn untag(&self, image_reference: &ImageReference) -> Result<(), ImageStoreError> {
        let db = &self.db;
        match &image_reference.tag {
            ImageTag::Tag(tag) => {
                let mut stmt = db.prepare_cached(
                    "delete from image_manifest_tags where hostname=? and name=? and tag=?",
                )?;
                let hostname = image_reference.hostname.clone().unwrap_or_default();
                stmt.execute((&hostname, &image_reference.name, &tag))?;
            }
            ImageTag::Digest(digest) => {
                let mut stmt = db.prepare_cached(
                    "delete from image_manifest_tags where hostname=? and name=? and digest=?",
                )?;
                let hostname = image_reference.hostname.clone().unwrap_or_default();
                stmt.execute((&hostname, &image_reference.name, digest.as_str()))?;
                let mut stmt = db.prepare_cached(
                    "delete from image_manifest_refs where hostname=? and name=? and digest=?",
                )?;
                stmt.execute((&hostname, &image_reference.name, digest.as_str()))?;
            }
        }
        Ok(())
    }

    fn list_all_tagged(&self) -> Result<Vec<ImageRecord>, ImageStoreError> {
        let mut stmt = self.db.prepare_cached(
            "
            select
                hostname, name, tag, image_manifests.digest, manifest
            from
                image_manifest_tags
            inner join
                image_manifests on image_manifests.digest = image_manifest_tags.digest
            ",
        )?;
        let mut rows = stmt.query([])?;
        let mut records = Vec::new();
        while let Ok(Some(row)) = rows.next() {
            let bytes: String = row.get(4)?;
            let manifest: JailImage = serde_json::from_str(&bytes)?;
            let hn: String = row.get(0)?;
            let image_reference = ImageReference {
                hostname: if hn.is_empty() { None } else { Some(hn) },
                name: row.get(1)?,
                tag: ImageTag::Tag(row.get(2)?),
            };
            records.push(ImageRecord {
                image_reference,
                digest: row.get(3)?,
                manifest,
            });
        }
        Ok(records)
    }

    fn list_all_tags(&self, name: &str) -> Result<Vec<ImageRecord>, ImageStoreError> {
        let mut stmt = self.db.prepare_cached(
            "
            select
                hostname, name, tag, image_manifests.digest, manifest
            from
                image_manifest_tags
            inner join
                image_manifests on image_manifests.digest = image_manifest_tags.digest
            where
                name=?
            ",
        )?;
        let mut rows = stmt.query([&name])?;
        let mut records = Vec::new();
        while let Ok(Some(row)) = rows.next() {
            let bytes: String = row.get(4)?;
            let manifest: JailImage = serde_json::from_str(&bytes)?;

            let hn: String = row.get(0)?;

            let image_reference = ImageReference {
                hostname: if hn.is_empty() { None } else { Some(hn) },
                name: row.get(1)?,
                tag: ImageTag::Tag(row.get(2)?),
            };

            records.push(ImageRecord {
                image_reference,
                digest: row.get(3)?,
                manifest,
            });
        }
        Ok(records)
    }

    fn list_all_manifests(&self) -> Result<HashMap<OciDigest, JailImage>, ImageStoreError> {
        let db = &self.db;
        let mut stmt = db.prepare_cached("select digest, manifest from image_manifests")?;
        let mut rows = stmt.query([])?;
        let mut ret = HashMap::new();
        while let Ok(Some(row)) = rows.next() {
            let digest_str: String = row.get(0)?;
            let digest = OciDigest::from_str(&digest_str)?;
            let bytes: String = row.get(1)?;
            let manifest: JailImage = serde_json::from_str(&bytes).unwrap();
            ret.insert(digest, manifest);
        }
        Ok(ret)
    }

    fn register_manifest(&self, manifest: &JailImage) -> Result<OciDigest, ImageStoreError> {
        let db = &self.db;
        let digest = manifest.digest();

        let mut stmt = db.prepare_cached(
            "insert into image_manifests (digest, manifest) values (?, ?)
                    on conflict(digest) do nothing",
        )?;
        let manifest_json = serde_json::to_string(manifest)?;
        stmt.execute([digest.as_str(), manifest_json.as_str()])?;
        Ok(digest)
    }

    fn tag_manifest(
        &self,
        digest: &OciDigest,
        image_reference: &ImageReference,
    ) -> Result<(), ImageStoreError> {
        let db = &self.db;
        let hostname = image_reference.hostname.clone().unwrap_or_default();
        let name = &image_reference.name;

        if let ImageTag::Tag(tag) = &image_reference.tag {
            let mut stmt = db.prepare_cached(
                "
                insert into image_manifest_tags (hostname, name, tag, digest) values (?, ?, ?, ?)
                    on conflict(hostname, name, tag) do update set digest=?",
            )?;

            stmt.execute((&hostname, name, tag, digest.as_str(), digest.as_str()))?;
        }

        let mut stmt = db.prepare_cached(
            "
            insert into image_manifest_refs (hostname, name, digest) values (?, ?, ?)
                on conflict(hostname, name, digest) do nothing
            ",
        )?;

        stmt.execute((&hostname, name, digest.as_str()))?;

        Ok(())
    }

    fn register_and_tag_manifest(
        &self,
        image_reference: &ImageReference,
        manifest: &JailImage,
    ) -> Result<OciDigest, ImageStoreError> {
        let digest = &self.register_manifest(manifest)?;
        self.tag_manifest(digest, image_reference)?;
        Ok(digest.clone())
    }

    fn query_manifest(
        &self,
        image_reference: &ImageReference,
    ) -> Result<ImageRecord, ImageStoreError> {
        if image_reference.tag.is_tag() {
            self.query_manifest_tagged(image_reference)
        } else {
            self.query_manifest_digest(image_reference)
        }
    }
}

impl SqliteImageStore {
    #[inline(always)]
    fn query_manifest_digest(
        &self,
        image_reference: &ImageReference,
    ) -> Result<ImageRecord, ImageStoreError> {
        let mut stmt = self.db.prepare_cached(
            "
            select
                hostname, name, image_manifests.digest, manifest
            from
                image_manifest_refs
            inner join
                image_manifests on image_manifests.digest = image_manifest_refs.digest
            where (hostname, name, image_manifest_refs.digest) = (?, ?, ?)",
        )?;

        let hostname = image_reference.hostname.clone().unwrap_or_default();
        let name = &image_reference.name;
        let tag = image_reference.tag.to_string();

        let manifest = stmt
            .query_row((&hostname, &name, &tag), |row| {
                let bytes: String = row.get(3)?;
                let manifest: JailImage = serde_json::from_str(&bytes).unwrap();
                let digest_str: String = row.get(2)?;

                let hn: String = row.get(0)?;

                let image_reference = ImageReference {
                    hostname: if hn.is_empty() { None } else { Some(hn) },
                    name: row.get(1)?,
                    tag: ImageTag::Digest(OciDigest::from_str(&digest_str).unwrap()),
                };

                Ok(ImageRecord {
                    image_reference,
                    digest: row.get(2)?,
                    manifest,
                })
            })
            .optional()?;

        match manifest {
            None => Err(ImageStoreError::TagNotFound(
                name.to_string(),
                tag.to_string(),
            )),
            Some(record) => Ok(record),
        }
    }

    #[inline(always)]
    fn query_manifest_tagged(
        &self,
        image_reference: &ImageReference,
    ) -> Result<ImageRecord, ImageStoreError> {
        let mut stmt = self.db.prepare_cached(
            "
            select
                hostname, name, tag, image_manifests.digest, manifest
            from
                image_manifest_tags
            inner join
                image_manifests on image_manifests.digest = image_manifest_tags.digest
            where (hostname, name, tag) = (?, ?, ?)",
        )?;

        let hostname = image_reference.hostname.clone().unwrap_or_default();
        let name = &image_reference.name;
        let tag = image_reference.tag.to_string();

        let manifest = stmt
            .query_row((&hostname, &name, &tag), |row| {
                let bytes: String = row.get(4)?;
                let manifest: JailImage = serde_json::from_str(&bytes).unwrap();
                let hn: String = row.get(0)?;

                let image_reference = ImageReference {
                    hostname: if hn.is_empty() { None } else { Some(hn) },
                    name: row.get(1)?,
                    tag: ImageTag::Tag(row.get(2)?),
                };

                Ok(ImageRecord {
                    image_reference,
                    digest: row.get(3)?,
                    manifest,
                })
            })
            .optional()?;

        match manifest {
            None => Err(ImageStoreError::TagNotFound(
                name.to_string(),
                tag.to_string(),
            )),
            Some(record) => Ok(record),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SqliteImageStore;
    use crate::image_store::{ImageRecord, ImageStore};
    use crate::models::jail_image::{JailConfig, JailImage};
    use oci_util::digest::OciDigest;
    use oci_util::image_reference::{ImageReference, ImageTag};
    use std::str::FromStr;

    #[test]
    fn test_image_store_diff_id_plain() {
        let db = SqliteImageStore::open_in_memory();
        let dummy = OciDigest::from_str(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        let _another_dummy = OciDigest::from_str(
            "sha256:deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        )
        .unwrap();
        db.create_tables().expect("cannot create tables");
        db.map_diff_id(&dummy, &dummy, "plain", None).unwrap();
        let yay = db
            .query_diff_id(&dummy)
            .expect("cannot query")
            .expect("should have result");
        assert_eq!(yay.diff_id, dummy);
    }

    #[test]
    fn test_image_store_diff_id_map() {
        let db = SqliteImageStore::open_in_memory();
        let dummy = OciDigest::from_str(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        let another_dummy = OciDigest::from_str(
            "sha256:deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        )
        .unwrap();
        db.create_tables().expect("cannot create tables");
        db.map_diff_id(&dummy, &another_dummy, "zstd", None)
            .unwrap();

        let yay = db
            .query_diff_id(&another_dummy)
            .expect("cannot query")
            .expect("should have result");
        let yay2 = db
            .query_diff_id(&dummy)
            .expect("cannot query")
            .expect("should have result");

        assert_eq!(yay.diff_id, dummy);
        assert_eq!(yay2.diff_id, dummy);

        db.map_diff_id(&dummy, &another_dummy, "zstd", None)
            .unwrap();
    }

    #[test]
    fn test_image_store_register_manifest() {
        let db = SqliteImageStore::open_in_memory();
        db.create_tables().expect("cannot create tables");
        let manifest = JailImage::default();
        let im = "test-name:test-tag".parse::<ImageReference>().unwrap();
        let digest = db
            .register_and_tag_manifest(&im, &manifest)
            .expect("cannot register and tag manifest");
        let records = db
            .list_all_tags("test-name")
            .expect("canont query all tags");
        let expected_record = ImageRecord {
            image_reference: im.clone(),
            digest: digest.to_string(),
            manifest,
        };
        eprintln!("records: {records:#?}");
        assert_eq!(records, vec![expected_record]);
    }

    #[test]
    fn test_image_store_register_manifest_query_by_digest() {
        let db = SqliteImageStore::open_in_memory();
        db.create_tables().expect("cannot create tables");
        let manifest = JailImage::default();
        let im = "test-name:test-tag".parse::<ImageReference>().unwrap();

        let digest = db
            .register_and_tag_manifest(&im, &manifest)
            .expect("cannot register and tag manifest");

        let imm = ImageReference {
            tag: ImageTag::Digest(digest.clone()),
            ..im
        };

        let records = db
            .query_manifest_digest(&imm)
            .expect("canont query manifest by digest");

        let expected_record = ImageRecord {
            image_reference: imm,
            digest: digest.to_string(),
            manifest,
        };
        assert_eq!(records, expected_record);
    }

    #[test]
    fn test_image_store_retag_manifest() {
        let im = "test-name:test-tag".parse::<ImageReference>().unwrap();
        let db = SqliteImageStore::open_in_memory();
        db.create_tables().expect("cannot create tables");
        let manifest1 = JailImage::default();

        let jail_config = JailConfig {
            linux: true,
            ..JailConfig::default()
        };

        let mut manifest2 = manifest1.clone();
        manifest2.set_config(&jail_config);
        db.register_and_tag_manifest(&im, &manifest1)
            .expect("cannot register and tag manifest");
        let digest = db.register_manifest(&manifest2).expect("");
        db.tag_manifest(&digest, &im).expect("");

        {
            let records = db
                .list_all_tags("test-name")
                .expect("canont query all tags");
            eprintln!("records: {records:#?}");
        }
        let manifest = db.query_manifest(&im).expect("cannot query manifest");

        assert_eq!(manifest.manifest, manifest2);
    }
}
