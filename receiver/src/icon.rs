/// Generate an RGBA icon at the given `size` (e.g. 32 for tray, 64 for window).
///
/// Draws a blue circle with a white microphone silhouette.
pub fn create_icon(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut data = vec![0u8; (size * size * 4) as usize];
    let cx = s / 2.0;
    let cy = s / 2.0;
    let bg_r = s * 0.44; // circle radius ≈ 44% of icon

    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let dx = fx - cx;
            let dy = fy - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist > bg_r + 0.5 {
                // Outside — transparent
                continue;
            }

            // Anti-alias edge of circle
            let circle_alpha = if dist > bg_r - 0.5 {
                1.0 - (dist - (bg_r - 0.5))
            } else {
                1.0
            };

            // Default: blue background
            let mut r = 64u8;
            let mut g = 154u8;
            let mut b = 242u8;

            // ----- Microphone shape (all coords relative to center) -----
            // Normalise to unit circle (-1..1)
            let nx = dx / bg_r;
            let ny = dy / bg_r;

            // Mic capsule: rounded rectangle  (upper portion)
            let cap_hw = 0.22; // half-width
            let cap_top = -0.55;
            let cap_bot = 0.10;
            let cap_r = 0.22; // corner radius = half-width → fully rounded ends

            let in_capsule = {
                let in_rect = nx.abs() <= cap_hw && ny >= cap_top && ny <= cap_bot;
                if in_rect {
                    // Check rounded corners
                    if ny < cap_top + cap_r {
                        // top corners
                        let corner_y = cap_top + cap_r;
                        let ddx = (nx.abs() - (cap_hw - cap_r)).max(0.0);
                        let ddy = corner_y - ny;
                        ddx * ddx + ddy * ddy <= cap_r * cap_r
                    } else if ny > cap_bot - cap_r {
                        // bottom corners
                        let corner_y = cap_bot - cap_r;
                        let ddx = (nx.abs() - (cap_hw - cap_r)).max(0.0);
                        let ddy = ny - corner_y;
                        ddx * ddx + ddy * ddy <= cap_r * cap_r
                    } else {
                        true
                    }
                } else {
                    false
                }
            };

            // Cradle arc (U-shape around lower half of capsule)
            let arc_cy_n = -0.05;
            let arc_r_outer = 0.42;
            let arc_r_inner = 0.32;
            let arc_dist = (nx * nx + (ny - arc_cy_n).powi(2)).sqrt();
            let in_arc = ny >= arc_cy_n
                && ny <= 0.38
                && arc_dist >= arc_r_inner
                && arc_dist <= arc_r_outer;

            // Stand (vertical bar)
            let in_stand = nx.abs() <= 0.06 && ny >= 0.35 && ny <= 0.55;

            // Base (horizontal bar)
            let in_base = nx.abs() <= 0.25 && ny >= 0.50 && ny <= 0.60;

            if in_capsule || in_arc || in_stand || in_base {
                r = 255;
                g = 255;
                b = 255;
            }

            let a = (circle_alpha * 255.0).round() as u8;
            data[idx] = r;
            data[idx + 1] = g;
            data[idx + 2] = b;
            data[idx + 3] = a;
        }
    }
    data
}
