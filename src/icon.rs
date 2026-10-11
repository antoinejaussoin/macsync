//! Menu-bar glyph: a clock face whose rim is a counter-clockwise arrow.
//! Drawn as black-on-transparent so macOS can tint it as a template image.

use std::f32::consts::PI;

pub const SIZE: u32 = 44;

pub fn rgba(alert: bool) -> Vec<u8> {
    let n = SIZE as usize;
    let mut out = vec![0u8; n * n * 4];
    const SAMPLES: usize = 4;
    for y in 0..n {
        for x in 0..n {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / n as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / n as f32;
                    if inside(px, py, alert) {
                        hits += 1;
                    }
                }
            }
            out[(y * n + x) * 4 + 3] = (hits * 255 / (SAMPLES * SAMPLES)) as u8;
        }
    }
    out
}

/// Coordinates are in [0, 1] with y pointing down.
fn inside(px: f32, py: f32, alert: bool) -> bool {
    let (cx, cy) = (0.5, 0.5);
    let (dx, dy) = (px - cx, cy - py);
    let r = (dx * dx + dy * dy).sqrt();
    let angle = dy.atan2(dx).rem_euclid(2.0 * PI).to_degrees();

    const RADIUS: f32 = 0.36;
    const RIM: f32 = 0.085;
    let (gap_start, gap_end) = (110.0, 150.0);
    let on_rim = (r - RADIUS).abs() <= RIM / 2.0 && !(gap_start..gap_end).contains(&angle);

    let at = |deg: f32, radius: f32| {
        let a = deg.to_radians();
        (cx + radius * a.cos(), cy - radius * a.sin())
    };
    let head = triangle(
        (px, py),
        at(gap_start, RADIUS - 0.13),
        at(gap_start, RADIUS + 0.13),
        at(gap_start + 32.0, RADIUS),
    );

    let minute_hand = segment_distance((px, py), (cx, cy), at(90.0, 0.22)) <= 0.04;
    let hour_hand = segment_distance((px, py), (cx, cy), at(-20.0, 0.16)) <= 0.04;

    let dot = alert && {
        let (ax, ay) = (0.8, 0.8);
        ((px - ax).powi(2) + (py - ay).powi(2)).sqrt() <= 0.17
    };
    let dot_halo = alert && ((px - 0.8).powi(2) + (py - 0.8).powi(2)).sqrt() <= 0.25;

    dot || (!dot_halo && (on_rim || head || minute_hand || hour_hand))
}

fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / (abx * abx + aby * aby)).clamp(0.0, 1.0);
    let (qx, qy) = (a.0 + t * abx, a.1 + t * aby);
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
}

fn triangle(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let sign =
        |p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)| (p1.0 - p3.0) * (p2.1 - p3.1) - (p2.0 - p3.0) * (p1.1 - p3.1);
    let (d1, d2, d3) = (sign(p, a, b), sign(p, b, c), sign(p, c, a));
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}
