//! End-to-end tests of the `stl2png` binary: argument handling, exit codes, output.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const EXAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/gear.stl");

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stl2png")).args(args).output().expect("failed to spawn stl2png")
}

fn stdout(o: &Output) -> String { String::from_utf8_lossy(&o.stdout).into_owned() }
fn stderr(o: &Output) -> String { String::from_utf8_lossy(&o.stderr).into_owned() }

/// A scratch directory unique to one test, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("stl2png-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn path(&self, file: &str) -> String { self.0.join(file).to_str().unwrap().to_owned() }
    fn write(&self, file: &str, data: &[u8]) -> String {
        let p = self.path(file);
        fs::write(&p, data).unwrap();
        p
    }
}

impl Drop for Scratch {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

/// Decode a PNG, returning (width, height, rgba bytes).
fn decode_png(path: &str) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(fs::File::open(path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

// ---- arguments --------------------------------------------------------------

#[test]
fn help_prints_usage_and_exits_zero() {
    for flag in ["-h", "--help"] {
        let o = run(&[flag]);
        assert_eq!(o.status.code(), Some(0));
        assert!(stdout(&o).starts_with("usage: stl2png"));
        assert!(stdout(&o).contains("--size"));
    }
}

#[test]
fn version_prints_crate_version() {
    for flag in ["-V", "--version"] {
        let o = run(&[flag]);
        assert_eq!(o.status.code(), Some(0));
        assert_eq!(stdout(&o).trim(), format!("stl2png {}", env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn wrong_number_of_files_is_a_usage_error() {
    for args in [&[][..], &["only.stl"][..], &["a.stl", "b.png", "c.png"][..], &["-s", "64"][..]] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(2), "args {args:?}");
        assert!(stderr(&o).starts_with("usage: stl2png"));
        assert!(stdout(&o).is_empty());
    }
}

#[test]
fn invalid_size_is_a_usage_error() {
    for size in ["0", "4097", "-1", "abc", "12.5", ""] {
        let o = run(&["-s", size, EXAMPLE, "/dev/null"]);
        assert_eq!(o.status.code(), Some(2), "size {size:?}");
        assert!(stderr(&o).contains("-s must be an integer between 1 and 4096"));
    }
    let o = run(&[EXAMPLE, "/dev/null", "-s"]);
    assert_eq!(o.status.code(), Some(2), "missing size value");
}

// ---- rendering the example ---------------------------------------------------

#[test]
fn renders_example_gear_at_default_size() {
    let s = Scratch::new("default");
    let out = s.path("gear.png");
    let o = run(&[EXAMPLE, &out]);
    assert_eq!(o.status.code(), Some(0), "stderr: {}", stderr(&o));
    assert!(stdout(&o).is_empty() && stderr(&o).is_empty());
    let (w, h, rgba) = decode_png(&out);
    assert_eq!((w, h), (256, 256));
    let opaque = rgba.chunks_exact(4).filter(|p| p[3] == 255).count();
    let transparent = rgba.chunks_exact(4).filter(|p| p[3] == 0).count();
    assert!(opaque > 256 * 256 / 5, "gear should cover a good part of the frame");
    assert!(transparent > 256 * 256 / 5, "background should stay transparent");
}

#[test]
fn size_flag_sets_output_dimensions() {
    let s = Scratch::new("size");
    for (flag, size) in [("-s", 2u32), ("--size", 17), ("-s", 96)] {
        let out = s.path(&format!("gear-{size}.png"));
        let o = run(&[flag, &size.to_string(), EXAMPLE, &out]);
        assert_eq!(o.status.code(), Some(0), "stderr: {}", stderr(&o));
        let (w, h, rgba) = decode_png(&out);
        assert_eq!((w, h), (size, size));
        assert!(rgba.chunks_exact(4).any(|p| p[3] > 0));
    }
}

#[test]
fn flags_may_come_after_positional_arguments() {
    let s = Scratch::new("order");
    let out = s.path("gear.png");
    let o = run(&[EXAMPLE, &out, "--size", "32"]);
    assert_eq!(o.status.code(), Some(0), "stderr: {}", stderr(&o));
    assert_eq!(decode_png(&out).0, 32);
}

#[test]
fn ascii_and_binary_input_render_identically() {
    let s = Scratch::new("ascii");
    let tri = "solid t\n facet normal 0 0 0\n  outer loop\n   vertex 0 0 0\n   vertex 10 0 0\n   vertex 0 0 10\n  endloop\n endfacet\n facet normal 0 0 0\n  outer loop\n   vertex 0 5 0\n   vertex 10 5 0\n   vertex 10 5 10\n  endloop\n endfacet\nendsolid t\n";
    let ascii = s.write("tri.stl", tri.as_bytes());

    let mut bin = vec![0u8; 80];
    bin.extend_from_slice(&2u32.to_le_bytes());
    for t in [[[0f32, 0., 0.], [10., 0., 0.], [0., 0., 10.]], [[0., 5., 0.], [10., 5., 0.], [10., 5., 10.]]] {
        bin.extend_from_slice(&[0; 12]);
        for p in t { for c in p { bin.extend_from_slice(&c.to_le_bytes()); } }
        bin.extend_from_slice(&[0; 2]);
    }
    let binary = s.write("tri-bin.stl", &bin);

    let (out_a, out_b) = (s.path("a.png"), s.path("b.png"));
    assert_eq!(run(&["-s", "48", &ascii, &out_a]).status.code(), Some(0));
    assert_eq!(run(&["-s", "48", &binary, &out_b]).status.code(), Some(0));
    assert_eq!(decode_png(&out_a), decode_png(&out_b));
}

#[test]
fn overwrites_existing_output_file() {
    let s = Scratch::new("overwrite");
    let out = s.write("gear.png", b"stale contents");
    assert_eq!(run(&["-s", "8", EXAMPLE, &out]).status.code(), Some(0));
    assert_eq!(decode_png(&out).0, 8);
}

// ---- failure modes -----------------------------------------------------------

#[test]
fn missing_input_file_exits_one() {
    let s = Scratch::new("missing");
    let o = run(&[&s.path("nope.stl"), &s.path("out.png")]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).starts_with("stl2png: "));
    assert!(stderr(&o).contains("nope.stl"));
    assert!(!std::path::Path::new(&s.path("out.png")).exists());
}

#[test]
fn empty_model_exits_one() {
    let s = Scratch::new("empty");
    let input = s.write("empty.stl", b"solid empty\nendsolid empty\n");
    let o = run(&[&input, &s.path("out.png")]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("no triangles found"));
    assert!(!std::path::Path::new(&s.path("out.png")).exists());
}

#[test]
fn malformed_ascii_exits_one_with_line_number() {
    let s = Scratch::new("malformed");
    let input = s.write("bad.stl", b"solid x\nvertex 1 2 3\nvertex 1 2 oops\nvertex 1 2 3\n");
    let o = run(&[&input, &s.path("out.png")]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("line 3: malformed vertex"));
}

#[test]
fn degenerate_model_exits_one_and_writes_nothing() {
    // All-collapsed triangles must not produce an empty thumbnail that the file
    // manager would cache.
    let s = Scratch::new("degenerate");
    let input = s.write("flat.stl", b"solid x\nvertex 1 1 1\nvertex 1 1 1\nvertex 1 1 1\nendsolid x\n");
    let out = s.path("out.png");
    let o = run(&[&input, &out]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("no renderable geometry"));
    assert!(!std::path::Path::new(&out).exists());
}

#[test]
fn unwritable_output_exits_one() {
    let s = Scratch::new("unwritable");
    let out = s.path("no-such-dir/out.png");
    let o = run(&[EXAMPLE, &out]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("no-such-dir"));
}
