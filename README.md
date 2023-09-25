# xc

##### This README last updated on 24th September, 2023

xc is a work-in-progress container engine for FreeBSD. This README document is intended for contributors, developers or anyone curious about this project to understand, build, run and contribute. This is not a step-by-step documentation, and such documentation (although is work-in-progress and potentially a bit outdated compare to the main branch) can be found [here](https://hackmd.io/7BIT_khIRQyPAe4EdiigHg)


## Overview

`xc` consists of 3 binaries, `xc` (client), `xcd` (server), `ocitar` (OCI layer archive helper). `xcd` depends on `ocitar` to run correctly, hence `ocitar` should exists in under one of `xcd`'s `$PATH` directories.

All three binaries can be built simply by runnig `cargo build` in the source directory.

#### Build Requirement

- FreeBSD 13 stable or newer (tested on both arm64 and amd64)
- cargo (require nightly if intended to build USDT probes for `xcd`)
- cmake (sqlite build dependency)


## Installation
Copy `xc`, `ocitar`, and `xcd` to one of the `$PATH` directories from build directories (`$src/target/release` for release build, `$src/target/debug` for debug build)

## Configuration
`xc` now take yaml configuration. The scheme (`struct XcConfig`) for the configuration file can be found at `xcd/src/config/mod.rs`. By default, `xcd` looks for the configuration at `/usr/local/etc/xc.config`

## Architecture

The core of `xc` is `xcd`, the daemon handles basically everything. `xc`, the client program, submit requests to the daemon via UNIX socket, typically at `/var/run/xc.sock`. Unlike similar container technology such as `docker`, `xcd` does not accept HTTP requests but instead accepts `JSON` encoded requests, sometimes with file descriptors. 

Every request `xcd` receives contains a method name and the corresponding payload. There are macros available to generate new methods to extend the features of `xc`. See `$src/xcd/src/ipc.rs` for examples.

The macro to define a new method also creates client-side helper functions `pub fn do_$method(..)` and can be used in the `xc` client program.

The global state of the daemon is called `Context`, and is defined in `$src/xcd/src/context.rs`.

### Containers

The global state (Context) owns a number of `Site`s. A `Site` is essentially an abstraction of "a place a container lives in". Think `Context` is a landlord, a `Site` is a portion of land the landlord rents out.

The purpose of this abstraction is to separate the duty of cleaning up a container. System-wise resources are made to clean up at the `Site` level, for example, destroying ZFS datasets, releasing IP addresses, etc, things that the teaent (container) shouldn't, and couldn't care about. This allows the global resources to always cleanup no matter what happened in the container to cause an exit (Jail cannot be created, precondition failure, executable crashed, cannot run the executable, etc...).

This is also planned to support FreeBSD containers that require multiple hosts to function in the future. More specifically, `Root-on-NFS` Jails, whose root filesystem may be exported by a different host than the host running the processes. In these cases, each host owns a site that references/relates to **one** container.

On the other hand. Once a site is created, the daemon process fork and run a kqueue backed run loop in the child process. This run-loop is responsible for spawning and reaping processes in the Jail, as well as collecting matrices. The site communicates with the run-loop via a UNIX socket pair, which sometimes also forwards file descriptors received from `xc` client to the run-loop. For example, in the case of `xc exec` without pty, the `stdout` and `stderr` file descriptors of the `xc` client process are first sent to the `xcd` daemon, which is later forwarded to the run-loop to use as the `stdout` and `stderr` of the new process.

Reaping is done by tracing the PIDs via `NOTE_TRACK` of `EVFILT_PROC` of `kqueue`. This allows us to reap processes without having an `init` in the Jail nor using `procctl`. The benefit of **not** using an `init` is to allow us only to track selections of process sub-trees that are directly related to the container lifetime. By doing this, we can prevent some long-running processes irrelevant to the container's lifetime (such as profiling/analytics) from stopping the container from exiting.

By default, unlike in `Docker`, `xc` waits for **all** descendants of the main process to exit before killing the container, instead of just the main process. In other words, processes such as `nginx` that immediately daemonize itself can run un-modified without special flags or `init`.
