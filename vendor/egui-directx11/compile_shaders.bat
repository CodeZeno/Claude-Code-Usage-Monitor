@cd shaders
fxc egui.hlsl /nologo /O3 /T vs_5_0 /E vs_egui /Fo vs_egui.bin
fxc egui.hlsl /nologo /O3 /T ps_5_0 /E ps_egui /Fo ps_egui.bin
@cd ..
