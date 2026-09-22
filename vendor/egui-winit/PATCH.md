# Text-only clipboard patch

Based on crates.io `egui-winit` 0.36.1, upstream commit
`4c1f2fae95475a40e524884ebb298bcb1714b08e` from
<https://github.com/emilk/egui/tree/4c1f2fae95475a40e524884ebb298bcb1714b08e/crates/egui-winit>.
The upstream MIT and Apache-2.0 licenses are included.

This application keeps native text copy/cut/paste but disables image clipboard
support to avoid enabling the `image` crate's BMP codec through `arboard`.

Changes from the published crate:

- Remove `arboard`'s `image-data` feature (its default features remain disabled).
- Remove `bytemuck` from the clipboard feature; other features can still enable it.
- Remove `Clipboard::set_image` and reject `OutputCommand::CopyImage` with a log
  message. The existing paste path reads text only.
- Point the manifest's license include paths to the bundled license files.

The root `[patch.crates-io]` entry selects this copy. When upgrading egui/eframe,
update this crate to the matching version and reapply these changes. Verify with
`cargo tree -e features -i image` and `cargo tree -e features -i arboard` that neither
`image/bmp` nor `arboard/image-data` is enabled, then build and test the application.
