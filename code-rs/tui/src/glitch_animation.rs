use ratatui::buffer::Buffer;
use ratatui::prelude::*;

#[derive(Clone, Copy)]
pub enum IntroColorMode {
    Rainbow,
    Gradient { start: Color, end: Color },
}

#[derive(Clone, Copy)]
struct GlyphPixel {
    ch: char,
    style: Style,
}

pub(crate) const SPARKSI_LIGHT_BLUE: Color = Color::Rgb(132, 188, 255);
pub(crate) const SPARKSI_LIME_GREEN: Color = Color::Rgb(181, 255, 92);

// Render the outline-fill animation
#[allow(dead_code)]
pub fn render_intro_animation(area: Rect, buf: &mut Buffer, t: f32) {
    // Avoid per-frame debug logging here to keep animation smooth.
    // (Heavy logging can starve the render loop on slower terminals.)
    render_intro_word_with_options(
        area,
        buf,
        t,
        None,
        "AVENUE",
        IntroColorMode::Rainbow,
        0,
        true,
    )
}

// Render the outline-fill animation with alpha blending for fade-out
#[allow(dead_code)]
pub fn render_intro_animation_with_alpha(area: Rect, buf: &mut Buffer, t: f32, alpha: f32) {
    render_intro_word_with_options(
        area,
        buf,
        t,
        Some(alpha),
        "AVENUE",
        IntroColorMode::Rainbow,
        0,
        true,
    )
}

// Public helper that allows callers to choose the rendered glyph string.
#[allow(dead_code)]
pub fn render_intro_animation_for_word(area: Rect, buf: &mut Buffer, t: f32, word: &str) {
    render_intro_word_with_options(
        area,
        buf,
        t,
        None,
        word,
        IntroColorMode::Rainbow,
        0,
        true,
    )
}

// Public helper that allows callers to choose the rendered glyph string with alpha blending.
#[allow(dead_code)]
pub fn render_intro_animation_with_alpha_for_word(
    area: Rect,
    buf: &mut Buffer,
    t: f32,
    alpha: f32,
    word: &str,
) {
    render_intro_word_with_options(
        area,
        buf,
        t,
        Some(alpha),
        word,
        IntroColorMode::Rainbow,
        0,
        true,
    )
}

pub(crate) fn render_intro_word_with_options(
    area: Rect,
    buf: &mut Buffer,
    t: f32,
    alpha: Option<f32>,
    word: &str,
    color_mode: IntroColorMode,
    offset: i32,
    clear_background: bool,
) {
    // Compute the final render rect first (including our 1‑col right shift)
    let mut r = area;
    if r.width > 0 {
        r.x = r.x.saturating_add(1);
        r.width = r.width.saturating_sub(1);
    }
    // Bail out early if the effective render rect is too small
    if r.width < 20 || r.height < 5 {
        tracing::warn!("!!! Area too small for animation: {}x{} (need 20x5)", r.width, r.height);
        return;
    }

    let t = t.clamp(0.0, 1.0);
    let outline_p = smoothstep(0.00, 0.60, t); // outline draws L->R
    let fill_p = smoothstep(0.35, 0.95, t); // interior fills L->R
    // Original fade profile: begin soft fade near the end.
    let fade = smoothstep(0.90, 1.00, t);
    let scan_p = smoothstep(0.55, 0.85, t); // scanline sweep
    let frame = (t * 60.0) as u32;

    // Build scaled mask + border map using the actual render rect size
    let (scale, mask, w, h) = scaled_mask(word, r.width, r.height);
    let border = compute_border(&mask);

    // Restrict height to the scaled glyph height
    r.height = h.min(r.height as usize) as u16;

    if clear_background {
        // Ensure background matches theme for the animation area
        let bg = crate::colors::background();
        for y in r.y..r.y.saturating_add(r.height) {
            for x in r.x..r.x.saturating_add(r.width) {
                let cell = &mut buf[(x, y)];
                cell.set_bg(bg);
                cell.set_char(' ');
            }
        }
    }

    let reveal_x_outline = (w as f32 * outline_p).round() as isize;
    let reveal_x_fill = (w as f32 * fill_p).round() as isize;
    let shine_x = (w as f32 * scan_p).round() as isize;
    let shine_band = scale.max(2) as isize;

    let pixels = mask_to_pixels(
        &mask,
        &border,
        reveal_x_outline,
        reveal_x_fill,
        shine_x,
        shine_band,
        fade,
        frame,
        scale,
        color_mode,
        alpha,
    );

    render_pixels(r, buf, &pixels, offset);
}

fn mask_to_pixels(
    mask: &Vec<Vec<bool>>,
    border: &Vec<Vec<bool>>,
    reveal_x_outline: isize,
    reveal_x_fill: isize,
    shine_x: isize,
    shine_band: isize,
    fade: f32,
    frame: u32,
    scale: usize,
    color_mode: IntroColorMode,
    alpha: Option<f32>,
) -> Vec<Vec<Option<GlyphPixel>>> {
    let h = mask.len();
    let w = mask[0].len();
    let mut out = Vec::with_capacity(h);

    // Gradient words should keep their vivid colors during the fade-out phase
    // so the color pop remains visible. We therefore suppress the mix-to-white
    // step for gradients.
    let fade_strength = match color_mode {
        IntroColorMode::Gradient { .. } => 0.0,
        _ => fade,
    };

    for y in 0..h {
        let mut row: Vec<Option<GlyphPixel>> = Vec::with_capacity(w);
        for x in 0..w {
            let xi = x as isize;

            let mut pixel = None;
            if mask[y][x] && xi <= reveal_x_fill {
                let base = base_color_for_column(x, w, color_mode);
                let dx = (xi - shine_x).abs();
                let shine =
                    (1.0 - (dx as f32 / (shine_band as f32 + 0.001)).clamp(0.0, 1.0)).powf(1.6);
                let bright = bump_rgb(base, shine * 0.30);
                // Make final state very light (almost invisible)
                let mut final_color = mix_rgb(bright, Color::Rgb(230, 232, 235), fade_strength);
                if let Some(alpha) = alpha {
                    final_color = blend_to_background(final_color, alpha);
                }
                pixel = Some(GlyphPixel {
                    ch: '█',
                    style: Style::default().fg(final_color).add_modifier(Modifier::BOLD),
                });
            } else if border[y][x] && xi <= reveal_x_outline.max(reveal_x_fill) {
                let base = base_color_for_column(x, w, color_mode);
                let period = (2 * scale_or(scale, 4)) as usize;
                let on = ((x + y + (frame as usize)) % period) < (period / 2);
                let base_with_ants = if on { bump_rgb(base, 0.22) } else { base };
                let mut final_color = mix_rgb(base_with_ants, Color::Rgb(235, 237, 240), fade_strength * 0.8);
                if let Some(alpha) = alpha {
                    final_color = blend_to_background(final_color, alpha);
                }
                pixel = Some(GlyphPixel {
                    ch: '▓',
                    style: Style::default().fg(final_color).add_modifier(Modifier::BOLD),
                });
            }

            row.push(pixel);
        }
        out.push(row);
    }

    out
}

fn render_pixels(area: Rect, buf: &mut Buffer, pixels: &[Vec<Option<GlyphPixel>>], offset: i32) {
    let base_x = area.x as i32;
    let base_y = area.y;
    let max_x = area.x.saturating_add(area.width) as i32;
    let max_y = area.y.saturating_add(area.height);

    for (row_idx, row) in pixels.iter().enumerate() {
        let y = base_y + row_idx as u16;
        if y >= max_y {
            break;
        }
        for (col_idx, maybe_pixel) in row.iter().enumerate() {
            let Some(pixel) = maybe_pixel else {
                continue;
            };
            let x = base_x + col_idx as i32 + offset;
            if x < area.x as i32 || x >= max_x {
                continue;
            }
            let cell = &mut buf[(x as u16, y)];
            cell.set_char(pixel.ch);
            cell.set_style(pixel.style);
        }
    }
}

fn base_color_for_column(x: usize, w: usize, color_mode: IntroColorMode) -> Color {
    match color_mode {
        IntroColorMode::Rainbow => gradient_multi(x as f32 / (w.max(1) as f32)),
        IntroColorMode::Gradient { start, end } => {
            let t = if w <= 1 { 0.0 } else { x as f32 / (w.saturating_sub(1) as f32) };
            mix_rgb(start, end, t)
        }
    }
}

// Helper function to blend colors towards background
pub(crate) fn blend_to_background(color: Color, alpha: f32) -> Color {
    if alpha >= 1.0 {
        return color;
    }
    if alpha <= 0.0 {
        return crate::colors::background();
    }

    let bg = crate::colors::background();

    match (color, bg) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let r = (r1 as f32 * alpha + r2 as f32 * (1.0 - alpha)) as u8;
            let g = (g1 as f32 * alpha + g2 as f32 * (1.0 - alpha)) as u8;
            let b = (b1 as f32 * alpha + b2 as f32 * (1.0 - alpha)) as u8;
            Color::Rgb(r, g, b)
        }
        _ => {
            // For non-RGB colors, just use alpha to decide between foreground and background
            if alpha > 0.5 { color } else { bg }
        }
    }
}

/* ---------------- border computation ---------------- */

fn compute_border(mask: &Vec<Vec<bool>>) -> Vec<Vec<bool>> {
    let h = mask.len();
    let w = mask[0].len();
    let mut out = vec![vec![false; w]; h];
    for y in 0..h {
        for x in 0..w {
            if !mask[y][x] {
                continue;
            }
            let up = y == 0 || !mask[y - 1][x];
            let down = y + 1 >= h || !mask[y + 1][x];
            let left = x == 0 || !mask[y][x - 1];
            let right = x + 1 >= w || !mask[y][x + 1];
            if up || down || left || right {
                out[y][x] = true;
            }
        }
    }
    out
}

/* ================= helpers ================= */

fn scale_or(scale: usize, min: usize) -> usize {
    if scale < min { min } else { scale }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

pub(crate) fn mix_rgb(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            Color::Rgb(lerp_u8(ar, br, t), lerp_u8(ag, bg, t), lerp_u8(ab, bb, t))
        }
        _ => b,
    }
}

// rainbow gradient sweeping red -> orange -> yellow -> green -> blue -> violet across the banner
pub(crate) fn gradient_multi(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    const STOPS: &[(u8, u8, u8)] = &[
        (255, 0, 0),     // red
        (255, 127, 0),   // orange
        (255, 255, 0),   // yellow
        (0, 200, 0),     // green
        (0, 80, 255),    // blue
        (158, 0, 255),   // violet
    ];

    if STOPS.len() == 1 {
        let (r, g, b) = STOPS[0];
        return Color::Rgb(r, g, b);
    }

    let segments = (STOPS.len() - 1) as f32;
    let scaled = t * segments;
    let idx = scaled.floor() as usize;
    let frac = (scaled - idx as f32).clamp(0.0, 1.0);
    let start_idx = idx.min(STOPS.len() - 1);
    let end_idx = (start_idx + 1).min(STOPS.len() - 1);
    let (sr, sg, sb) = STOPS[start_idx];
    let (er, eg, eb) = STOPS[end_idx];
    Color::Rgb(
        lerp_u8(sr, er, frac),
        lerp_u8(sg, eg, frac),
        lerp_u8(sb, eb, frac),
    )
}

fn bump_rgb(c: Color, amt: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            let add = |x: u8| ((x as f32 + 255.0 * amt).min(255.0)) as u8;
            Color::Rgb(add(r), add(g), add(b))
        }
        _ => c,
    }
}

// Scale a 5×7 word bitmap (e.g., "CODE") to fill `max_w` x `max_h`, returning (scale, grid, w, h)
fn scaled_mask(word: &str, max_w: u16, max_h: u16) -> (usize, Vec<Vec<bool>>, usize, usize) {
    let rows = 7usize;
    let w = 5usize;
    let gap = 1usize;
    let letters: Vec<[&'static str; 7]> = word.chars().map(glyph_5x7).collect();
    let cols = letters.len() * w + (letters.len().saturating_sub(1)) * gap;

    // Start with an even smaller scale to prevent it from getting massive on wide terminals
    let mut scale = 3usize;
    while scale > 1 && (cols * scale > max_w as usize || rows * scale > max_h as usize) {
        scale -= 1;
    }
    if scale == 0 {
        scale = 1;
    }

    let mut grid = vec![vec![false; cols * scale]; rows * scale];
    let mut xoff = 0usize;

    for g in letters {
        for row in 0..rows {
            let line = g[row].as_bytes();
            for col in 0..w {
                if line[col] == b'#' {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            grid[row * scale + dy][(xoff + col) * scale + dx] = true;
                        }
                    }
                }
            }
        }
        xoff += w + gap;
    }
    (scale, grid, cols * scale, rows * scale)
}

// 5×7 glyphs for supported characters (capital letters + space)
fn glyph_5x7(ch: char) -> [&'static str; 7] {
    match ch {
        'A' => [
            " ### ",
            "#   #",
            "#   #",
            "#####",
            "#   #",
            "#   #",
            "#   #",
        ],
        'C' => [
            " ### ", "#   #", "#    ", "#    ", "#    ", "#   #", " ### ",
        ],
        'K' => [
            "#   #",
            "#  # ",
            "# #  ",
            "##   ",
            "# #  ",
            "#  # ",
            "#   #",
        ],
        'O' => [
            " ### ", "#   #", "#   #", "#   #", "#   #", "#   #", " ### ",
        ],
        'P' => [
            "#### ",
            "#   #",
            "#   #",
            "#### ",
            "#    ",
            "#    ",
            "#    ",
        ],
        'U' => [
            "#   #",
            "#   #",
            "#   #",
            "#   #",
            "#   #",
            "#   #",
            " ### ",
        ],
        'S' => [
            " ### ",
            "#   #",
            "#    ",
            " ### ",
            "    #",
            "#   #",
            " ### ",
        ],
        'T' => [
            "#####",
            "  #  ",
            "  #  ",
            "  #  ",
            "  #  ",
            "  #  ",
            "  #  ",
        ],
        'D' => [
            "#### ", "#   #", "#   #", "#   #", "#   #", "#   #", "#### ",
        ],
        'E' => [
            "#####", "#    ", "#    ", "#####", "#    ", "#    ", "#####",
        ],
        'N' => [
            "#   #",
            "##  #",
            "# # #",
            "#  ##",
            "#   #",
            "#   #",
            "#   #",
        ],
        'R' => [
            "#### ", "#   #", "#   #", "#### ", "# #  ", "#  # ", "#   #",
        ],
        'I' => [
            "#####",
            "  #  ",
            "  #  ",
            "  #  ",
            "  #  ",
            "  #  ",
            "#####",
        ],
        'V' => [
            "#   #",
            "#   #",
            "#   #",
            "#   #",
            " # # ",
            " # # ",
            "  #  ",
        ],
        ' ' => [
            "     ",
            "     ",
            "     ",
            "     ",
            "     ",
            "     ",
            "     ",
        ],
        _ => [
            "#####", "#####", "#####", "#####", "#####", "#####", "#####",
        ],
    }
}
