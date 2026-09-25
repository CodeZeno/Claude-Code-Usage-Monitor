# Local D3D11 painter adaptation

Source: https://github.com/NekomaruQwQ/egui-directx11
Commit: 38201f9b9620a26c3e3740775fcaf24487f1dde1 (MIT OR Apache-2.0).
Only the library, licenses, shaders and shader compilation script are vendored.

- Use egui 0.36 without default fonts; the application supplies its font subsets.
- Consume egui 0.36's ordered batches of updates for each texture.
- Apply pixels_per_point once (it already includes user zoom).
- Clamp scissor rectangles to the render target.
- Apply texture frees after drawing, including empty frames.
- Use DEFAULT textures and UpdateSubresource for rectangular updates; do not
  assume mapped RowPitch equals width * 4. Validate update bounds.
- Honor each texture's filtering and wrap options; skip missing textures.
- Shaders remain the upstream precompiled shader-model-5 binaries. End users
  need neither a compiler DLL nor a DirectX SDK. Source and rebuild script are included.
