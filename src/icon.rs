//! The app icon, drawn in code so the window icon (at runtime) and the .exe icon
//! (embedded by build.rs) come from one place: a dark tile with three coloured
//! "rows" in the same colours the grid uses for dev, docker and app listeners.
#![allow(dead_code)] // build.rs uses `ico`, the app uses `rgba`

const RING: [u8; 3] = [0x5B, 0x60, 0x78];
const TILE: [u8; 3] = [0x24, 0x27, 0x3A];
const ROWS: [[u8; 3]; 3] = [[0xA6, 0xDA, 0x95], [0x7D, 0xC4, 0xE4], [0xCA, 0xD3, 0xF5]];

/// Signed distance from (px, py) to a rounded rectangle, all in unit coordinates.
fn sd_round_rect(px: f32, py: f32, (x0, y0, x1, y1): (f32, f32, f32, f32), r: f32) -> f32 {
    let qx = (px - (x0 + x1) / 2.0).abs() - ((x1 - x0) / 2.0 - r);
    let qy = (py - (y0 + y1) / 2.0).abs() - ((y1 - y0) / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// Straight-alpha RGBA pixels, `size` x `size`, top row first.
pub fn rgba(size: u32) -> Vec<u8> {
    let mut shapes = vec![
        ((0.03, 0.03, 0.97, 0.97), 0.23, RING),
        ((0.075, 0.075, 0.925, 0.925), 0.19, TILE),
    ];
    for (i, color) in ROWS.iter().enumerate() {
        let cy = 0.31 + 0.19 * i as f32;
        let h = 0.065;
        shapes.push(((0.20, cy - h, 0.33, cy + h), h, *color));
        shapes.push(((0.41, cy - h, 0.80, cy + h), h, *color));
    }

    let s = size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = ((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
            let (mut rgb, mut a) = ([0.0f32; 3], 0.0f32);
            for &(rect, r, color) in &shapes {
                let cover = (0.5 - sd_round_rect(px, py, rect, r) * s).clamp(0.0, 1.0);
                let na = cover + a * (1.0 - cover);
                if na > 0.0 {
                    for c in 0..3 {
                        rgb[c] = (color[c] as f32 * cover + rgb[c] * a * (1.0 - cover)) / na;
                    }
                }
                a = na;
            }
            out.extend([rgb[0] as u8, rgb[1] as u8, rgb[2] as u8, (a * 255.0).round() as u8]);
        }
    }
    out
}

/// A Windows .ico holding one 32-bit BMP image per size.
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&size| {
            let px = rgba(size);
            let mask_row = size.div_ceil(32) * 4;
            let mut bmp = Vec::new();
            for v in [40u32, size, size * 2] {
                bmp.extend(v.to_le_bytes());
            }
            bmp.extend(1u16.to_le_bytes()); // planes
            bmp.extend(32u16.to_le_bytes()); // bits per pixel
            bmp.extend(0u32.to_le_bytes()); // BI_RGB
            bmp.extend((size * size * 4 + mask_row * size).to_le_bytes());
            bmp.extend([0u8; 16]);
            for y in (0..size).rev() {
                for x in 0..size {
                    let i = ((y * size + x) * 4) as usize;
                    bmp.extend([px[i + 2], px[i + 1], px[i], px[i + 3]]); // BGRA, bottom-up
                }
            }
            bmp.extend(vec![0u8; (mask_row * size) as usize]); // AND mask unused with alpha
            bmp
        })
        .collect();

    let mut out = Vec::new();
    out.extend([0u8, 0, 1, 0]);
    out.extend((sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&size, img) in sizes.iter().zip(&images) {
        let dim = if size >= 256 { 0 } else { size as u8 };
        out.extend([dim, dim, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((img.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend(img);
    }
    out
}
