# OCI-tar

This tool handles creating and extracting file system layers like the [OCI filesystem layer](https://github.com/opencontainers/image-spec/blob/main/layer.md).

Filesystem layers contain differentiation (creation, modification, and deletion of files) between states of a file system. Creating and modifying files are trivial to accomplish using tar but not deletion. The OCI specification uses special whiteout files to represent the deletion of files from the parent layer. In addition to how OCI handles it, this tool can write a custom pax header containing all paths to whiteout. The extra header does not affect most tar implementations to list and extract the archive.

# Usage


### Creating a layer

```shell=
# the following command create an archive from [folder1, folder2, folder3], and 
# contains the deletion of "folder/0"
ocitar -cf mylayer.tar --remove folder/0 folder1 folder2 folder3
```

This creates a OCI compatible layer that archived `folder1` `folder2` `folder3` with whiteout file `folder/.wh.0` included.

Output to stdout is also supported

```shell=
ocitar -cf - --remove folder/0 folder1 folder2 folder3
```

### Stage a layer
Given an OCI compatible layer `mylayer.tar` created in previous session. The following command extract the layer to `myfolder`, including the deletions.

```shell=
ocitar -xf mylayer.tar -C myfolder
```

Reading the layer from stdin can be done by
```shell=
ocitar -xf- -C myfolder
```

### Integration with ZFS
In addition to the common usages, creating a layer from the difference between ZFS datasets is also supported.
```shell=
# Create a layer containing difference between `zroot/my_dataset@eariler` and `zroot/my_dataset`
ocitar -cf mylayer.tar --zfs-diff zroot/my_dataset@eariler zroot/my_dataset
```

### Compression
This tool support creating and extracting layers compressed with ZStandard and Gzip.

Creating and extracting compressed layer can be done by adding `--compression=$type` to the argument list. Available options for `$type` are `zstd` and `gzip`.
