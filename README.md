# xc
##### This README last updated on 31st March, 2024

xc is a work-in-progress container engine for FreeBSD that is capable to run FreeBSD containers and Linux (Docker) containers. Unlike `podman`, this project is intend to be more FreeBSD focused and made adjustment to support and integrate with FreeBSD specific features, such as `Pf`, `ZFS`, `DTrace` and `Jail`(including nested jails/containers and VNET/Non-VNET networking).

Please scroll to [Quick Start](#quick-start) session for usage. To under start `xc` a bit more and how everything is put together, please jump to [In-depth documentation](#in-depth-documentation) session.

A more detailed (but bit outdated) documentation can be found [here](https://hackmd.io/7BIT_khIRQyPAe4EdiigHg)

# Quick Start

This session covers how to use [xc](https://github.com/michael-yuji/xc), the FreeBSD container engine.

`xc` is not yet available as a port, so you'll have to build and install by yourself. Luckily, the process is easy.

## Requirement:
- FreeBSD 14 (amd64 or arm64)
- ZFS
- Enable pf

## Build and install
Run the following commands. If you prefer `doas` over `sudo`, feel free to use it.
```shell
sudo pkg install git cmake rust
git clone https://github.com/michael-yuji/xc.git
cd xc && cargo build --release
sudo cp target/release/xcd /usr/local/sbin
sudo cp target/release/{ocitar, xc} /usr/local/bin
```

## Configuration
Before configuring `xc`. Give the following questions a thought:
- which network interface should serve as the external interface for your containers
- in which ZFS pool/dataset you wish the data lives in

Assume the answer of your first question in `em0`, and for the second question `zroot/xc`, create a yaml file at `/usr/local/etc/xc.conf`.

```yaml
# for published ports, for example `-p 80:8080`, only packets reaching
# the `ext_ifs` forwards to the container(s) by default
ext_ifs: 
    - em0

# dataset to store the container images, each image becomes a child dataset
image_dataset: zroot/xc/datasets

# dataset contains the root of running containers
container_dataset: zroot/xc/run

# dataset serving volumes
default_volume_dataset: zroot/xc/volumes
```

### Make network accessible

Part of the core value of `xc` is to bring operator no surprise, hence it is not going to configure networking/firewall setting for you automatically.

Instead, `xc` creates entries, allocate and set ip addresses, insert forwarding rules to anchors you provides explicitly. Therefore, we need to add some rules to our pf configuartion to forward packets.

First, ensure that the host can forward packets. To make the following command preserve across reboots, add `gateway_enable="YES"` to `rc.conf`.

`sysctl net.inet.ip.forwarding=1`

Now, create following to `pf.conf`. We are assuming to use `em0` to serve internet traffic for our containers.

```
ext_if="em0"
nat on $ext_if from <xc:network:default> to any -> ($ext_if)
rdr-anchor xc-rdr
```

You may have notice the pf table `<xc:network:default>`. We are going to create an xc network named `default` later. Whenever a container request an address from this network, `xc` insert the address to this table such that the `nat` rule will work.

Similarly, `xc` insert its port forwarding rules to `xc-rdr`. loading this anchor causes the forwarding rules created by `xc` to work.

### Creating network interface

```shell
ifconfig bridge create name xc0
ifconfig xc0 172.16.0.254/24 up
```

As mentioned before, `xc` never invade your network without your explicit setup, instead you tell `xc` where you want the containers' connectivities should go. We are going to create the `default` network later, and all containers using the `default` network will either create an alias on this interface (if it's a non-VNET container), or create a `epair` interface and add as a member to this bridge.

## Starting up

In a different terminal, or in a tmux session, start `xcd` as root.
`sudo xcd`

### Creating our first network
As mentioned in the previous session, we are going to create our first network for our containers. Let's call it `default`.

`xc create network --alias xc0 --bridge xc0 --default-router 172.16.0.254 default 172.16.0.0/24`

This tells xc to create a network that
- named `default`
- with address pool of `172.16.0.0/24`, `xc` can automatically allocate address from this pool if an explicit address is not specified
- when create a non-VNET jail, create the IP alias on `xc0`
- when create a VNET jail, bridge the `epair` interface to `xc0`

### Create our first FreeBSD container

First pull the container image
```
xc pull freebsdxc/freebsd:13.2
```
Now run the container. A network-less container is useless, so let's make it a VNET jail (`--vnet`) and attach to the network we created earlier, `default`:
```
xc run -it --vnet --network default -- /bin/sh
```

Now you have your very first container to play with.

### Using Linux containers from DockerHub

`xc` can run Linux containers via FreeBSD Linuxulator.

First you need to configure your host to support that, that includes loading the required kernel modules and a sysctl to make the kernel run unknown ELF binary as Linux binaries (go binaries does that).

Load the following kernel modules: `kldload linux64 linprocfs linsysfs`

You may want to run `kldload linux` as well to run i386 Linux containers.

Modify the sysctl: `kern.elf64.fallback_brand=3`

#### Try!

Pull the Linux mariadb 10.9 image from dockerhub:
```shell
xc pull library/mariadb:10.9
```

Run it:
```shell
xc run -e MARIADB_ROOT_PASSWORD=password library/mariadb:10.9
```

# In-Depth documentation
🚧 under construction and refinement 🚧 

## Configuration
`xc` now take yaml configuration. The scheme (`struct XcConfig`) for the configuration file can be found at `xcd/src/config/mod.rs`. By default, `xcd` looks for the configuration at `/usr/local/etc/xc.config`

## Architecture

The core of `xc` is `xcd`, the daemon handles basically everything. `xc`, the client program, submit requests to the daemon via UNIX socket, typically at `/var/run/xc.sock`. Unlike similar container technology such as `docker`, `xcd` does not accept HTTP requests but instead accepts `JSON` encoded requests, sometimes with file descriptors. 

Every request `xcd` receives contains a method name and the corresponding payload. There are macros available to generate new methods to extend the features of `xc`. See `$src/xcd/src/ipc.rs` for examples.

The macro to define a new method also creates client-side helper functions `pub fn do_$method(..)` and can be used in the `xc` client program.

The global state of the daemon is called `Context`, and is defined in `$src/xcd/src/context.rs`.

### Containers

The global state (Context) owns a number of `Site`s. A `Site` is essentially an abstraction of "a place a container lives in". Think `Context` is a landlord, a `Site` is a portion of land the landlord rents out.

The purpose of this abstraction is to separate the duty of cleaning up a container. System-wise resources are made to clean up at the `Site` level, for example, destroying ZFS datasets, releasing IP addresses, etc, things that the tenant (container) shouldn't, and couldn't care about. This allows the global resources to always cleanup no matter what happened in the container to cause an exit (Jail cannot be created, precondition failure, executable crashed, cannot run the executable, etc...).

This is also planned to support FreeBSD containers that require multiple hosts to function in the future. More specifically, `Root-on-NFS` Jails, whose root filesystem may be exported by a different host than the host running the processes. In these cases, each host owns a site that references/relates to **one** container.

On the other hand. Once a site is created, the daemon process fork and run a kqueue backed run loop in the child process. This run-loop is responsible for spawning and reaping processes in the Jail, as well as collecting matrices. The site communicates with the run-loop via a UNIX socket pair, which sometimes also forwards file descriptors received from `xc` client to the run-loop. For example, in the case of `xc exec` without pty, the `stdout` and `stderr` file descriptors of the `xc` client process are first sent to the `xcd` daemon, which is later forwarded to the run-loop to use as the `stdout` and `stderr` of the new process.

Reaping is done by tracing the PIDs via `NOTE_TRACK` of `EVFILT_PROC` of `kqueue`. This allows us to reap processes without having an `init` in the Jail nor using `procctl`. The benefit of **not** using an `init` is to allow us only to track selections of process sub-trees that are directly related to the container lifetime. By doing this, we can prevent some long-running processes irrelevant to the container's lifetime (such as profiling/analytics) from stopping the container from exiting.

By default, unlike in `Docker`, `xc` waits for **all** descendants of the main process to exit before killing the container, instead of just the main process. In other words, processes such as `nginx` that immediately daemonize itself can run un-modified without special flags or `init`.
