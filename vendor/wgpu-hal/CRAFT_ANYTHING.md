# wgpu-hal 30.0.1, patched for Craft Anything

This is a copy of `wgpu-hal` 30.0.1 (MIT OR Apache-2.0, see `LICENSE.*`) with one change, used through
`[patch.crates-io]` in the workspace `Cargo.toml`.

## The change

`src/metal/adapter.rs`: the Metal shader language version is no longer taken from the macOS version
alone. `accepted_msl_version` compiles a tiny probe kernel with each version up to the OS's, highest
first, and keeps the first one the system's Metal compiler accepts. If none is accepted it keeps the
OS's version, as upstream does.

## Why

On some Macs the OS expects a Metal language version its compiler does not accept. On iMac 2012 Macs
(GT 640M, macOS 15 under OpenCore Legacy Patcher) macOS 15 selects `metal3.2` and the compiler stops at
`metal3.1`. Every shader fails, the device is lost and the app's window never opens.

- Craft Anything issue #742.
- Upstream: gfx-rs/wgpu #10687 (closed as a duplicate, no fix on trunk), #10592 (open).

## Removing it

Delete this directory and the `[patch.crates-io]` entry once a released wgpu picks a Metal version the
compiler accepts (check that the 2012 iMac case in #742 works on that release first).
