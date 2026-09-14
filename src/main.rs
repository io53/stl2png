//! stl2png — tiny CPU-only STL thumbnailer. No GPU, no window system.
//! Usage: stl2png [-s SIZE] input.stl output.png

use std::fs;
use std::io::BufWriter;
use std::process::exit;

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: V3, b: V3) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 { [a[0] / l, a[1] / l, a[2] / l] } else { [0.0, 0.0, 1.0] }
}

/// Load triangles (3 vertices each) from binary or ASCII STL.
fn load_stl(path: &str) -> Result<Vec<V3>, String> {
    let data = fs::read(path).map_err(|e| e.to_string())?;
    let v = parse_stl(&data)?;
    if v.is_empty() {
        return Err("no triangles found".into());
    }
    Ok(v)
}

fn parse_stl(data: &[u8]) -> Result<Vec<V3>, String> {
    // Binary: 80-byte header, u32 triangle count, 50 bytes per triangle. Trust the
    // count if the file is at least that long and doesn't look like ASCII; some
    // exporters append junk after the last triangle.
    if data.len() >= 84 {
        let n = u32::from_le_bytes([data[80], data[81], data[82], data[83]]) as usize;
        let expected = n.checked_mul(50).and_then(|x| x.checked_add(84));
        let is_ascii = data.starts_with(b"solid") && !expected.is_some_and(|e| e == data.len());
        if let (Some(expected), false) = (expected, is_ascii) {
            if n > 0 && data.len() >= expected {
                let mut v = Vec::with_capacity(n * 3);
                for t in 0..n {
                    let base = 84 + t * 50 + 12; // skip stored normal
                    for i in 0..3 {
                        let o = base + i * 12;
                        let f = |k: usize| f32::from_le_bytes(data[o + k..o + k + 4].try_into().unwrap());
                        v.push([f(0), f(4), f(8)]);
                    }
                }
                return Ok(v);
            }
        }
    }
    let text = String::from_utf8_lossy(data);
    let mut v = Vec::new();
    for (ln, line) in text.lines().enumerate() {
        let mut it = line.split_whitespace();
        if it.next().is_some_and(|w| w.eq_ignore_ascii_case("vertex")) {
            let mut p = [0f32; 3];
            for c in &mut p {
                *c = it
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| format!("line {}: malformed vertex", ln + 1))?;
            }
            v.push(p);
        }
    }
    if v.len() % 3 != 0 {
        return Err(format!("vertex count {} is not a multiple of 3", v.len()));
    }
    Ok(v)
}

/// Fixed three-quarter camera: azimuth around Z (up), then tilt so we look down
/// at the model. View space: x right, z up, y into the screen.
struct View { ca: f32, sa: f32, ct: f32, st: f32 }

impl View {
    fn new() -> Self {
        let (az, tilt) = (-35f32.to_radians(), 30f32.to_radians());
        View { ca: az.cos(), sa: az.sin(), ct: tilt.cos(), st: tilt.sin() }
    }
    fn apply(&self, p: V3) -> V3 {
        let (x, y) = (p[0] * self.ca - p[1] * self.sa, p[0] * self.sa + p[1] * self.ca);
        let z = p[2];
        [x, y * self.ct - z * self.st, y * self.st + z * self.ct]
    }
}

/// Supersampling factor: the model is rasterized at `SS`× the output size and
/// box-filtered down, with coverage becoming alpha.
const SS: usize = 2;

/// Render triangles (3 vertices each) into a `size`×`size` RGBA8 buffer with a
/// transparent background. Triangles with non-finite coordinates are dropped.
/// Returns `None` if nothing ended up on screen.
fn render(verts: &[V3], size: usize) -> Option<Vec<u8>> {
    let cam = View::new();
    let view: Vec<V3> = verts.chunks_exact(3)
        .filter(|t| t.iter().flatten().all(|c| c.is_finite()))
        .flatten()
        .map(|&p| cam.apply(p))
        .collect();

    // Fit to frame.
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &view {
        for i in 0..3 { lo[i] = lo[i].min(p[i]); hi[i] = hi[i].max(p[i]); }
    }
    let w = size * SS;
    let extent = (hi[0] - lo[0]).max(hi[2] - lo[2]).max(1e-6);
    let scale = w as f32 * 0.92 / extent;
    let cx = (lo[0] + hi[0]) * 0.5;
    let cz = (lo[2] + hi[2]) * 0.5;
    let half = w as f32 * 0.5;
    let to_screen = |p: V3| -> V3 {
        [(p[0] - cx) * scale + half, half - (p[2] - cz) * scale, p[1]]
    };

    // Rasterize with a z-buffer and flat Lambert shading.
    let light = norm([-0.45, -0.35, 0.82]);
    let base = [0.62f32, 0.72, 0.90];
    let mut zbuf = vec![f32::MAX; w * w];
    let mut col = vec![[0f32; 3]; w * w];
    let mut hit = vec![false; w * w];
    for tri in view.chunks_exact(3) {
        let n = norm(cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])));
        let n = if n[1] > 0.0 { [-n[0], -n[1], -n[2]] } else { n }; // face the camera
        let shade = 0.22 + 0.78 * dot(n, light).max(0.0);
        let c = [base[0] * shade, base[1] * shade, base[2] * shade];
        let a = to_screen(tri[0]);
        let b = to_screen(tri[1]);
        let d = to_screen(tri[2]);
        let area = (b[0] - a[0]) * (d[1] - a[1]) - (b[1] - a[1]) * (d[0] - a[0]);
        if area.abs() < 1e-12 { continue; }
        let x0 = a[0].min(b[0]).min(d[0]).floor().max(0.0) as usize;
        let x1 = (a[0].max(b[0]).max(d[0]).ceil() as usize).min(w - 1);
        let y0 = a[1].min(b[1]).min(d[1]).floor().max(0.0) as usize;
        let y1 = (a[1].max(b[1]).max(d[1]).ceil() as usize).min(w - 1);
        for y in y0..=y1 {
            let py = y as f32 + 0.5;
            for x in x0..=x1 {
                let px = x as f32 + 0.5;
                let w0 = ((b[0] - px) * (d[1] - py) - (b[1] - py) * (d[0] - px)) / area;
                let w1 = ((d[0] - px) * (a[1] - py) - (d[1] - py) * (a[0] - px)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 { continue; }
                let z = w0 * a[2] + w1 * b[2] + w2 * d[2];
                let i = y * w + x;
                if z < zbuf[i] { zbuf[i] = z; col[i] = c; hit[i] = true; }
            }
        }
    }

    if !hit.iter().any(|&h| h) {
        return None;
    }

    // Downsample to the final size; alpha = coverage.
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let (mut acc, mut n) = ([0f32; 3], 0u32);
            for dy in 0..SS {
                for dx in 0..SS {
                    let i = (y * SS + dy) * w + x * SS + dx;
                    if hit[i] { n += 1; for k in 0..3 { acc[k] += col[i][k]; } }
                }
            }
            if n > 0 {
                let o = (y * size + x) * 4;
                for k in 0..3 { out[o + k] = (acc[k] / n as f32 * 255.0).round().clamp(0.0, 255.0) as u8; }
                out[o + 3] = (n * 255 / (SS * SS) as u32) as u8;
            }
        }
    }
    Some(out)
}

/// Write a `size`×`size` RGBA8 buffer as a PNG file.
fn write_png(path: &str, size: usize, rgba: &[u8]) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(BufWriter::new(file), size as u32, size as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(rgba).and_then(|_| w.finish()))
        .map_err(|e| e.to_string())
}

const USAGE: &str = "usage: stl2png [-s SIZE] input.stl output.png\n\n  -s, --size SIZE  output width and height in pixels (1-4096, default 256)\n  -h, --help       show this help\n  -V, --version    show version";
const MAX_SIZE: usize = 4096;

fn main() {
    let mut size = 256usize;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => { println!("{USAGE}"); exit(0); }
            "-V" | "--version" => { println!("stl2png {}", env!("CARGO_PKG_VERSION")); exit(0); }
            "-s" | "--size" => {
                size = match args.next().and_then(|s| s.parse::<usize>().ok()) {
                    Some(n) if (1..=MAX_SIZE).contains(&n) => n,
                    _ => { eprintln!("stl2png: -s must be an integer between 1 and {MAX_SIZE}"); exit(2); }
                }
            }
            _ => files.push(a),
        }
    }
    if files.len() != 2 {
        eprintln!("{USAGE}");
        exit(2);
    }
    let verts = match load_stl(&files[0]) {
        Ok(v) => v,
        Err(e) => { eprintln!("stl2png: {}: {e}", files[0]); exit(1); }
    };
    let out = match render(&verts, size) {
        Some(o) => o,
        None => { eprintln!("stl2png: {}: model has no renderable geometry", files[0]); exit(1); }
    };
    if let Err(e) = write_png(&files[1], size, &out) {
        eprintln!("stl2png: {}: {e}", files[1]);
        exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- fixtures ---------------------------------------------------------

    /// Build a binary STL from triangles, with the given header and trailing bytes.
    fn binary_stl(header: &[u8], tris: &[[V3; 3]], trailer: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(header);
        d.resize(80, 0);
        d.extend_from_slice(&(tris.len() as u32).to_le_bytes());
        for t in tris {
            d.extend_from_slice(&[0u8; 12]); // normal, ignored
            for p in t {
                for c in p { d.extend_from_slice(&c.to_le_bytes()); }
            }
            d.extend_from_slice(&[0u8; 2]); // attribute byte count
        }
        d.extend_from_slice(trailer);
        d
    }

    /// Build an ASCII STL from triangles.
    fn ascii_stl(tris: &[[V3; 3]]) -> String {
        let mut s = String::from("solid test\n");
        for t in tris {
            s.push_str("  facet normal 0 0 0\n    outer loop\n");
            for p in t {
                s.push_str(&format!("      vertex {} {} {}\n", p[0], p[1], p[2]));
            }
            s.push_str("    endloop\n  endfacet\n");
        }
        s.push_str("endsolid test\n");
        s
    }

    const TRI: [V3; 3] = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 0.0, 10.0]];
    const TRI2: [V3; 3] = [[1.5, -2.0, 3.25], [4.0, 5.0, 6.0], [-7.0, 8.0, 9.5]];

    fn flat(tris: &[[V3; 3]]) -> Vec<V3> { tris.iter().flatten().copied().collect() }

    fn pixel(img: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        let o = (y * size + x) * 4;
        [img[o], img[o + 1], img[o + 2], img[o + 3]]
    }

    fn alpha_count(img: &[u8]) -> usize { img.chunks_exact(4).filter(|p| p[3] > 0).count() }

    // ---- vector math ------------------------------------------------------

    #[test]
    fn vector_ops() {
        assert_eq!(sub([1.0, 2.0, 3.0], [0.5, 1.0, 1.5]), [0.5, 1.0, 1.5]);
        assert_eq!(dot([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), 32.0);
        assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(cross([0.0, 1.0, 0.0], [1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]);
    }

    #[test]
    fn norm_scales_to_unit_length() {
        let n = norm([3.0, 0.0, 4.0]);
        assert!((n[0] - 0.6).abs() < 1e-6 && n[1] == 0.0 && (n[2] - 0.8).abs() < 1e-6);
        let n = norm([1.0, 1.0, 1.0]);
        assert!((dot(n, n) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn norm_of_zero_vector_is_up() {
        assert_eq!(norm([0.0, 0.0, 0.0]), [0.0, 0.0, 1.0]);
    }

    // ---- STL parsing ------------------------------------------------------

    #[test]
    fn parses_binary_stl() {
        let data = binary_stl(b"binary header", &[TRI, TRI2], b"");
        assert_eq!(parse_stl(&data).unwrap(), flat(&[TRI, TRI2]));
    }

    #[test]
    fn binary_stl_tolerates_trailing_junk() {
        let data = binary_stl(b"", &[TRI], b"some junk appended by an exporter");
        assert_eq!(parse_stl(&data).unwrap(), flat(&[TRI]));
    }

    #[test]
    fn binary_stl_whose_header_starts_with_solid_is_still_binary() {
        // Some exporters write "solid" in the binary header. If the triangle count
        // matches the file size exactly, treat it as binary.
        let data = binary_stl(b"solid exported-by-foo", &[TRI, TRI2], b"");
        assert_eq!(parse_stl(&data).unwrap(), flat(&[TRI, TRI2]));
    }

    #[test]
    fn binary_stl_starting_with_solid_and_trailing_junk_falls_back_to_ascii() {
        // Ambiguous: looks like ASCII and the count doesn't match the size. The
        // ASCII path finds no "vertex" lines, so the result is empty rather than
        // garbage triangles.
        let data = binary_stl(b"solid", &[TRI], b"junk");
        assert!(parse_stl(&data).unwrap().is_empty());
    }

    #[test]
    fn truncated_binary_stl_is_not_trusted() {
        let mut data = binary_stl(b"", &[TRI, TRI2], b"");
        data.truncate(data.len() - 1);
        // Count says 2 triangles but the file is too short: falls through to the
        // ASCII parser, which finds nothing.
        assert!(parse_stl(&data).unwrap().is_empty());
    }

    #[test]
    fn binary_stl_with_huge_count_does_not_overflow() {
        let mut data = vec![0u8; 84];
        data[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
        data.extend_from_slice(&[0u8; 50]);
        assert!(parse_stl(&data).unwrap().is_empty());
    }

    #[test]
    fn binary_stl_with_zero_count_yields_nothing() {
        let data = binary_stl(b"", &[], b"");
        assert_eq!(data.len(), 84);
        assert!(parse_stl(&data).unwrap().is_empty());
    }

    #[test]
    fn parses_ascii_stl() {
        let data = ascii_stl(&[TRI, TRI2]);
        assert!(data.len() >= 84, "fixture should exercise the binary sniffing path");
        assert_eq!(parse_stl(data.as_bytes()).unwrap(), flat(&[TRI, TRI2]));
    }

    #[test]
    fn parses_short_ascii_stl() {
        let data = "vertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\n";
        assert!(data.len() < 84);
        assert_eq!(parse_stl(data.as_bytes()).unwrap(), vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    }

    #[test]
    fn ascii_stl_is_case_insensitive_and_accepts_exponents() {
        let data = "SOLID x\nVERTEX 1e1 -2.5E-1 +3\nVertex 0 0 0\nvertex\t1\t2\t3\nENDSOLID x\n";
        assert_eq!(parse_stl(data.as_bytes()).unwrap(), vec![[10.0, -0.25, 3.0], [0.0, 0.0, 0.0], [1.0, 2.0, 3.0]]);
    }

    #[test]
    fn ascii_stl_ignores_extra_tokens_after_coordinates() {
        let data = "vertex 1 2 3 extra\nvertex 4 5 6\nvertex 7 8 9\n";
        assert_eq!(parse_stl(data.as_bytes()).unwrap().len(), 3);
    }

    #[test]
    fn ascii_stl_reports_malformed_vertex_with_line_number() {
        let data = "solid x\nvertex 1 2 3\nvertex 4 five 6\nvertex 7 8 9\n";
        assert_eq!(parse_stl(data.as_bytes()).unwrap_err(), "line 3: malformed vertex");
        let data = "vertex 1 2\n";
        assert_eq!(parse_stl(data.as_bytes()).unwrap_err(), "line 1: malformed vertex");
    }

    #[test]
    fn ascii_stl_rejects_vertex_count_not_multiple_of_three() {
        let data = "vertex 1 2 3\nvertex 4 5 6\n";
        assert_eq!(parse_stl(data.as_bytes()).unwrap_err(), "vertex count 2 is not a multiple of 3");
    }

    #[test]
    fn empty_and_textual_input_yield_no_triangles() {
        assert!(parse_stl(b"").unwrap().is_empty());
        assert!(parse_stl(b"solid empty\nendsolid empty\n").unwrap().is_empty());
        assert!(parse_stl("not an stl at all \u{fffd}\u{1f600}".as_bytes()).unwrap().is_empty());
    }

    #[test]
    fn invalid_utf8_does_not_panic() {
        let mut data = b"vertex 1 2 3\n".to_vec();
        data.extend_from_slice(&[0xff, 0xfe, 0xc0]);
        data.extend_from_slice(b"\nvertex 4 5 6\nvertex 7 8 9\n");
        assert_eq!(parse_stl(&data).unwrap().len(), 3);
    }

    #[test]
    fn load_stl_errors_on_missing_file_and_empty_model() {
        assert!(load_stl("/nonexistent/path/to/model.stl").is_err());
        let dir = std::env::temp_dir().join(format!("stl2png-unit-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("empty.stl");
        fs::write(&p, "solid e\nendsolid e\n").unwrap();
        assert_eq!(load_stl(p.to_str().unwrap()).unwrap_err(), "no triangles found");
        let p = dir.join("ok.stl");
        fs::write(&p, binary_stl(b"", &[TRI], b"")).unwrap();
        assert_eq!(load_stl(p.to_str().unwrap()).unwrap(), flat(&[TRI]));
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- view transform ---------------------------------------------------

    #[test]
    fn view_transform_preserves_length_and_looks_down_from_above() {
        let cam = View::new();
        for p in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [3.0, -4.0, 12.0]] {
            let q = cam.apply(p);
            assert!((dot(p, p) - dot(q, q)).abs() < 1e-4, "rotation should preserve length");
        }
        assert_eq!(cam.apply([0.0; 3]), [0.0; 3]);
        // Model-space "up" stays mostly up on screen, and tilts away from the camera.
        let up = cam.apply([0.0, 0.0, 1.0]);
        assert!(up[2] > 0.8 && up[1] < 0.0);
        assert!((up[1] + 0.5).abs() < 1e-6, "30° tilt: sin(30°) = 0.5");
    }

    // ---- rendering --------------------------------------------------------

    #[test]
    fn render_produces_rgba_buffer_of_requested_size() {
        for size in [1usize, 2, 7, 64] {
            let img = render(&flat(&[TRI]), size).expect("triangle should render");
            assert_eq!(img.len(), size * size * 4);
            assert!(alpha_count(&img) > 0);
        }
    }

    #[test]
    fn render_leaves_background_transparent_black() {
        let size = 64;
        let img = render(&flat(&[TRI]), size).unwrap();
        let mut transparent = 0;
        for p in img.chunks_exact(4) {
            if p[3] == 0 {
                assert_eq!(&p[..3], &[0, 0, 0]);
                transparent += 1;
            }
        }
        assert!(transparent > 0, "a single triangle cannot cover the whole frame");
        assert!(transparent < size * size);
    }

    #[test]
    fn render_fits_model_inside_frame_with_margin() {
        let size = 64;
        let img = render(&flat(&[TRI, TRI2]), size).unwrap();
        for i in 0..size {
            for (x, y) in [(i, 0), (i, size - 1), (0, i), (size - 1, i)] {
                assert_eq!(pixel(&img, size, x, y)[3], 0, "border pixel ({x},{y}) should be empty");
            }
        }
        // ...but the model should still fill a decent part of the frame.
        assert!(alpha_count(&img) > size * size / 10);
    }

    #[test]
    fn render_interior_pixels_are_fully_opaque_and_edges_partially_covered() {
        let size = 64;
        // A big square facing the camera head-on in model space.
        let q = [[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [1.0, 0.0, 1.0]];
        let r = [[-1.0, 0.0, -1.0], [1.0, 0.0, 1.0], [-1.0, 0.0, 1.0]];
        let img = render(&flat(&[q, r]), size).unwrap();
        let alphas: Vec<u8> = img.chunks_exact(4).map(|p| p[3]).collect();
        assert!(alphas.contains(&255), "interior pixels should be fully covered");
        assert!(alphas.iter().any(|&a| a > 0 && a < 255), "edge pixels should be anti-aliased");
        // Alpha is coverage out of SS*SS subsamples, so only these values can occur.
        for a in alphas {
            assert!([0, 63, 127, 191, 255].contains(&a), "unexpected alpha {a}");
        }
    }

    #[test]
    fn render_uses_bluish_base_colour() {
        let img = render(&flat(&[TRI, TRI2]), 64).unwrap();
        for p in img.chunks_exact(4).filter(|p| p[3] == 255) {
            assert!(p[0] < p[1] && p[1] < p[2], "expected r < g < b, got {:?}", &p[..3]);
            assert!(p[2] > 0);
        }
    }

    #[test]
    fn render_is_deterministic() {
        let verts = flat(&[TRI, TRI2]);
        assert_eq!(render(&verts, 48).unwrap(), render(&verts, 48).unwrap());
    }

    #[test]
    fn render_is_independent_of_model_position_and_scale() {
        let size = 64;
        let base = render(&flat(&[TRI, TRI2]), size).unwrap();
        let moved: Vec<V3> = flat(&[TRI, TRI2]).iter()
            .map(|p| [p[0] * 4.0 + 1000.0, p[1] * 4.0 - 250.0, p[2] * 4.0 + 8.0])
            .collect();
        let img = render(&moved, size).unwrap();
        // Floating point rounding may move an edge pixel or two, so compare coverage
        // and colour statistically rather than byte-for-byte.
        let (a, b) = (alpha_count(&base) as f32, alpha_count(&img) as f32);
        assert!((a - b).abs() / a < 0.02, "coverage {a} vs {b}");
        let opaque = |img: &[u8]| img.chunks_exact(4).filter(|p| p[3] == 255).map(|p| [p[0], p[1], p[2]]).collect::<std::collections::BTreeSet<_>>();
        assert_eq!(opaque(&base), opaque(&img), "flat shading colours should match");
    }

    #[test]
    fn render_flips_normals_that_face_away_from_camera() {
        // Reversing the winding order must not change the shading.
        let size = 64;
        let fwd = render(&flat(&[TRI, TRI2]), size).unwrap();
        let rev = render(&flat(&[[TRI[0], TRI[2], TRI[1]], [TRI2[0], TRI2[2], TRI2[1]]]), size).unwrap();
        let opaque = |img: &[u8]| img.chunks_exact(4).filter(|p| p[3] == 255).map(|p| [p[0], p[1], p[2]]).collect::<std::collections::BTreeSet<_>>();
        assert_eq!(opaque(&fwd), opaque(&rev));
        assert!((alpha_count(&fwd) as i64 - alpha_count(&rev) as i64).abs() <= 2);
    }

    #[test]
    fn render_shades_by_orientation() {
        // The same triangle at two different orientations gets two different shades.
        let a = render(&flat(&[TRI]), 32).unwrap();
        let tilted = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 7.0, 7.0]];
        let b = render(&flat(&[tilted]), 32).unwrap();
        let colour = |img: &[u8]| img.chunks_exact(4).find(|p| p[3] == 255).map(|p| [p[0], p[1], p[2]]).unwrap();
        assert_ne!(colour(&a), colour(&b));
    }

    #[test]
    fn render_zbuffer_keeps_nearest_surface() {
        let size = 64;
        let cam = View::new();
        // Two overlapping triangles with different orientations, one in front of the other.
        let near = [[-3.0, 0.0, -3.0], [3.0, 0.0, -3.0], [0.0, 0.0, 3.0]];
        let far = [[-20.0, 30.0, -20.0], [20.0, 30.0, -20.0], [0.0, 45.0, 25.0]];
        let depth = |t: &[V3; 3]| t.iter().map(|&p| cam.apply(p)[1]).sum::<f32>() / 3.0;
        assert!(depth(&near) < depth(&far), "fixture: `near` must be closer to the camera");

        let colour = |img: &[u8]| img.chunks_exact(4).find(|p| p[3] == 255).map(|p| [p[0], p[1], p[2]]).unwrap();
        let (near_only, far_only) = (render(&flat(&[near]), size).unwrap(), render(&flat(&[far]), size).unwrap());
        assert_ne!(colour(&near_only), colour(&far_only), "fixture: shades must differ");

        // Rendering both with the far one first: the far one's screen area contains the
        // near one's, so the near colour must survive in the middle of the image.
        let both = render(&flat(&[far, near]), size).unwrap();
        let both_swapped = render(&flat(&[near, far]), size).unwrap();
        assert_eq!(both, both_swapped, "draw order must not matter");
        let colours: std::collections::BTreeSet<_> = both.chunks_exact(4).filter(|p| p[3] == 255).map(|p| [p[0], p[1], p[2]]).collect();
        assert!(colours.contains(&colour(&near_only)), "near surface was overdrawn");
        assert!(colours.contains(&colour(&far_only)));
    }

    #[test]
    fn render_returns_none_for_degenerate_geometry() {
        let point = [[1.0, 2.0, 3.0]; 3];
        assert!(render(&flat(&[point]), 32).is_none());
        let line = [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [2.0, 2.0, 2.0]];
        assert!(render(&flat(&[line]), 32).is_none());
        assert!(render(&[], 32).is_none());
    }

    #[test]
    fn render_drops_triangles_with_non_finite_coordinates() {
        let nan = [[f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let inf = [[0.0, 0.0, 0.0], [f32::INFINITY, 0.0, 0.0], [0.0, 0.0, 1.0]];
        assert!(render(&flat(&[nan]), 32).is_none());
        assert!(render(&flat(&[inf]), 32).is_none());
        // Bad triangles don't poison the bounding box for the good ones.
        let good = render(&flat(&[TRI]), 32).unwrap();
        let mixed = render(&flat(&[nan, TRI, inf]), 32).unwrap();
        assert_eq!(good, mixed);
    }

    #[test]
    fn render_handles_tiny_and_huge_models() {
        let tiny: Vec<V3> = flat(&[TRI]).iter().map(|p| [p[0] * 1e-5, p[1] * 1e-5, p[2] * 1e-5]).collect();
        let huge: Vec<V3> = flat(&[TRI]).iter().map(|p| [p[0] * 1e6, p[1] * 1e6, p[2] * 1e6]).collect();
        assert!(alpha_count(&render(&tiny, 32).unwrap()) > 10);
        assert!(alpha_count(&render(&huge, 32).unwrap()) > 10);
    }

    // ---- PNG output -------------------------------------------------------

    #[test]
    fn write_png_round_trips() {
        let dir = std::env::temp_dir().join(format!("stl2png-png-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.png");
        let size = 16;
        let img = render(&flat(&[TRI, TRI2]), size).unwrap();
        write_png(path.to_str().unwrap(), size, &img).unwrap();

        let decoder = png::Decoder::new(fs::File::open(&path).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (16, 16));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        assert_eq!(&buf[..info.buffer_size()], &img[..]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_png_reports_unwritable_path() {
        let err = write_png("/nonexistent-dir/out.png", 4, &[0; 64]).unwrap_err();
        assert!(!err.is_empty());
    }
}
