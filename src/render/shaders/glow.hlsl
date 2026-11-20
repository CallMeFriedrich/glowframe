// Glow frame (SPEC 5.5 v1.1): a comet - bright head with a soft leading edge
// and a long fading tail - travels clockwise along the rounded screen border at
// uniform perimeter speed; the rest keeps a faint base glow. The glow falls off
// softly towards the inside and reaches deeper around the head.

cbuffer Params : register(b0)
{
    float2 size;        // pixels
    float  time;        // seconds
    float  phase;       // head position 0..1 (fraction of the perimeter)
    float4 c0;          // palette, linear sRGB (head -> tail end)
    float4 c1;
    float4 c2;
    float  brightness;  // overall multiplier (incl. breathing)
    float  width;       // falloff length in pixels
    float  beat;        // beat pulse 0..1 (already scaled by the setting)
    float  energy;      // smoothed energy 0..1 (already scaled)
    float  corner;      // corner radius in pixels
    float  base_level;  // glow outside the comet (0.05)
    float  tail_len;    // tail length (fraction of the perimeter)
    float  dither_seed;
    float  head_len;    // head size (gaussian width, fraction of the perimeter)
    float  lead_len;    // soft leading edge in front of the head
    float2 comet_w;     // weights of comet 2 and 3 (v1.4, one per dominant color)
    float  spread;      // distance between the comets (fraction of the perimeter)
    float3 pad1;
};

float4 vs_main(uint id : SV_VertexID) : SV_Position
{
    // Fullscreen triangle.
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
}

// Signed distance to a rounded rectangle centered at the origin (inside < 0).
float sd_round_rect(float2 p, float2 b, float r)
{
    float2 q = abs(p) - b + r;
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r;
}

// Perimeter coordinate 0..1 (clockwise from top center) of the point where the
// ray from the center through p meets the border. Continuous everywhere except
// at the exact center, so the arc has no seams inside the screen.
float perimeter_coord(float2 p, float2 b, float r)
{
    float2 av = abs(p) + 1e-4;
    float t = min(b.x / av.x, b.y / av.y);
    float2 h = av * t;
    float straight_x = b.x - r;
    float straight_y = b.y - r;
    float quarter = straight_x + 1.5707963 * r + straight_y;

    float q;
    if (h.x > straight_x && h.y > straight_y && r > 0.0)
    {
        // Ray hits the corner arc: intersect with the corner circle.
        float2 c = float2(straight_x, straight_y);
        float2 d = normalize(av);
        float bq = dot(d, c);
        float cq = dot(c, c) - r * r;
        float tt = bq + sqrt(max(bq * bq - cq, 0.0));
        float2 hc = d * tt - c;
        float a = atan2(hc.x, hc.y); // 0 at the top, pi/2 at the side
        q = straight_x + clamp(a, 0.0, 1.5707963) * r;
    }
    else if (h.y >= b.y - 1e-3)
    {
        q = h.x; // top/bottom edge
    }
    else
    {
        q = straight_x + 1.5707963 * r + (straight_y - h.y); // side edge
    }

    // Screen y points down: top half has p.y < 0.
    float s;
    if (p.x >= 0.0)
        s = (p.y < 0.0) ? q : 2.0 * quarter - q;
    else
        s = (p.y >= 0.0) ? 2.0 * quarter + q : 4.0 * quarter - q;
    return s / (4.0 * quarter);
}

float3 palette3(float x)
{
    x = saturate(x) * 2.0;
    return x < 1.0 ? lerp(c0.rgb, c1.rgb, x) : lerp(c1.rgb, c2.rgb, x - 1.0);
}

// One comet with its head at `ph`: head and tail intensity, tail position.
void comet_at(float s, float ph, out float head, out float tail, out float tail_t)
{
    // The head moves towards increasing s (clockwise); `behind` is the
    // distance back along the tail.
    float behind = frac(ph - s);
    float ahead = frac(s - ph);
    head = max(exp(-(behind * behind) / (head_len * head_len)),
               exp(-(ahead * ahead) / (lead_len * lead_len)));
    // Points just ahead of the head belong to the head (no color seam there).
    tail_t = (ahead < behind) ? 0.0 : saturate(behind / tail_len);
    tail = (ahead < behind) ? 0.0 : pow(1.0 - tail_t, 1.5) * 0.82;
}

float3 linear_to_srgb(float3 c)
{
    c = saturate(c);
    return c <= 0.0031308 ? c * 12.92 : 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    float2 half_size = size * 0.5;
    float2 p = pos.xy - half_size;

    float sdf = sd_round_rect(p, half_size, corner);
    float coverage = saturate(0.5 - sdf);        // black outside the corners

    // Comets along the perimeter: the first one always, a second and third
    // one (weights comet_w) evenly spaced behind it when the cover has
    // several dominant colors.
    float s = perimeter_coord(p, half_size, corner);
    float h0, t0, tt0, h1, t1, tt1, h2, t2, tt2;
    comet_at(s, phase, h0, t0, tt0);
    comet_at(s, frac(phase - spread), h1, t1, tt1);
    comet_at(s, frac(phase - 2.0 * spread), h2, t2, tt2);
    float i0 = max(h0, t0);
    float i1 = max(h1, t1) * comet_w.x;
    float i2 = max(h2, t2) * comet_w.y;
    float head = max(h0, max(h1 * comet_w.x, h2 * comet_w.y));
    float comet = max(i0, max(i1, i2));
    float along = lerp(base_level, 1.0, comet);

    // Single comet: color runs from the head (c0) to the tail end (c2) and
    // settles into the palette middle for the base glow (no seam opposite
    // the head). Several comets: each one in its own color, the base glow
    // in their mix.
    float multi = max(comet_w.x, comet_w.y);
    float3 col0 = lerp(palette3(tt0), c0.rgb, multi);
    float3 comet_color = (col0 * i0 + c1.rgb * i1 + c2.rgb * i2) / max(i0 + i1 + i2, 1e-4);
    float3 mix_color = (c0.rgb + c1.rgb * comet_w.x + c2.rgb * comet_w.y)
                     / (1.0 + comet_w.x + comet_w.y);
    float3 base_color = lerp(c1.rgb, mix_color, multi);
    float3 color = lerp(base_color, comet_color, smoothstep(0.0, 0.35, comet));

    // Falloff: soft glow plus a bright core line at the edge. Summing the
    // exponential per edge is a soft minimum of the edge distances, which
    // avoids diagonal creases towards the corners. Around the head the light
    // reaches further into the screen.
    float w = width * (1.0 + 0.35 * beat) * (1.0 + 0.3 * head);
    float wc = max(w * 0.14, 1.0);
    float4 de = float4(pos.x, size.x - pos.x, pos.y, size.y - pos.y);
    float glow = dot(exp(-de / w), 0.6) + dot(exp(-de / wc), 1.5);
    glow = min(glow, 2.2);

    float level = brightness * (1.0 + 0.6 * beat) * (0.92 + 0.16 * energy) * (1.0 + 0.35 * head);
    float3 c = color * glow * along * level * coverage;

    // Hue-preserving highlight compression, only the hottest core whitens a bit.
    float m = max(max(c.r, c.g), c.b);
    if (m > 1e-5)
    {
        float mm = 1.0 - exp(-m);
        c *= mm / m;
        c = lerp(c, float3(mm, mm, mm), 0.25 * mm * mm * mm * mm);
    }

    float3 srgb = linear_to_srgb(c);
    float n = frac(sin(dot(pos.xy, float2(12.9898, 78.233)) + dither_seed) * 43758.5453) - 0.5;
    return float4(srgb + n / 255.0, 1.0);
}
