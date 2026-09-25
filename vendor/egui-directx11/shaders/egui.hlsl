void vs_egui(
    in const float2 i_pos  : POSITION,
    in const float2 i_uv   : TEXCOORD,
    in const float4 i_color: COLOR,
    out      float4 o_pos  : SV_POSITION,
    out      float2 o_uv   : TEXCOORD,
    out      float4 o_color: COLOR) {
    o_pos   = float4(i_pos, 0.0, 1.0);
    o_uv    = i_uv;
    o_color = i_color;
}

Texture2D<float4> g_texture: register(t0);
SamplerState      g_sampler: register(s0);

float4 ps_egui(
    in const float4 i_pos  : SV_POSITION,
    in const float2 i_uv   : TEXCOORD,
    in const float4 i_color: COLOR): SV_TARGET {
    return i_color * g_texture.SampleLevel(g_sampler, i_uv, 0);
}
