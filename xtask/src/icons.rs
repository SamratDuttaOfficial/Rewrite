//! `cargo xtask icons [--font <bold serif .ttf>]`: the app icon, drawn from its parts and rendered at every
//! size the three systems use.
//!
//! The icon is a rounded square in the app's accent blue, a white serif "R" and the yellow underline of the
//! old icon. The R is the outline of the font's "R" (Georgia Bold by default), written into
//! `assets/icons/rewrite.svg` as a path, so later renders need no font. The PNGs go to
//! `assets/icons/hicolor/<n>x<n>/apps/rewrite.png`; the .ico and .icns are made from them when packaging.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// The sizes rendered: Windows (16 to 256; 20 and 40 are the small and large icons at 125%), Linux (hicolor),
/// macOS (up to 1024 for Retina).
pub const SIZES: [u32; 11] = [16, 20, 24, 32, 40, 48, 64, 128, 256, 512, 1024];

const BLUE: &str = "#3B5BDB";
const YELLOW: &str = "#FFD43B";

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let svg_path = root.join("assets/icons/rewrite.svg");
    let font = args.windows(2).find(|w| w[0] == "--font").map(|w| PathBuf::from(&w[1]));
    if font.is_some() || !svg_path.is_file() {
        let font = font.unwrap_or_else(default_font);
        let data = fs::read(&font).map_err(|e| format!("{}: {e} (pass --font <bold serif .ttf>)", font.display()))?;
        fs::write(&svg_path, svg(&data)?).map_err(|e| e.to_string())?;
        println!("[xtask] wrote {}", svg_path.display());
    }
    let svg = fs::read_to_string(&svg_path).map_err(|e| e.to_string())?;
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).map_err(|e| e.to_string())?;
    let mut dirs = vec![];
    for n in SIZES {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(n, n).ok_or("pixmap")?;
        let k = n as f32 / 256.0;
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
        let dir = root.join(format!("assets/icons/hicolor/{n}x{n}/apps"));
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        pixmap.save_png(dir.join("rewrite.png")).map_err(|e| e.to_string())?;
        dirs.push(format!("{n}x{n}/apps"));
    }
    let mut index = format!("[Icon Theme]\nName=hicolor\nDirectories={}\n", dirs.join(","));
    for n in SIZES {
        let _ = write!(index, "\n[{n}x{n}/apps]\nSize={n}\nType=Fixed\n");
    }
    fs::write(root.join("assets/icons/hicolor/index.theme"), index).map_err(|e| e.to_string())?;
    println!("[xtask] rendered the icon at {SIZES:?} pixels");
    Ok(())
}

fn default_font() -> PathBuf {
    let candidates = [
        "C:/Windows/Fonts/georgiab.ttf",
        "/System/Library/Fonts/Supplemental/Georgia Bold.ttf",
        "/Library/Fonts/Georgia Bold.ttf",
        "/usr/share/fonts/truetype/msttcorefonts/Georgia_Bold.ttf",
    ];
    candidates.iter().map(PathBuf::from).find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(candidates[0]))
}

/// The icon on a 256 x 256 canvas, measured from the earlier PNG icon.
fn svg(font: &[u8]) -> Result<String, String> {
    let face = ttf_parser::Face::parse(font, 0).map_err(|e| format!("not a font: {e}"))?;
    let gid = face.glyph_index('R').ok_or("the font has no R")?;
    let mut path = PathData::default();
    let bbox = face.outline_glyph(gid, &mut path).ok_or("the R has no outline")?;
    // the R fills (62, 75) - (183, 192), as it did in the PNG icon
    let (x0, y0, x1, y1) = (62.0, 75.0, 183.0, 192.0);
    let k = ((x1 - x0) / bbox.width() as f64).min((y1 - y0) / bbox.height() as f64);
    let tx = x0 - bbox.x_min as f64 * k + ((x1 - x0) - bbox.width() as f64 * k) / 2.0;
    let ty = y1 + bbox.y_min as f64 * k; // font units go up; the canvas goes down
    Ok(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <rect x="8" y="8" width="240" height="240" rx="56" fill="{BLUE}"/>
  <path transform="matrix({k:.6} 0 0 {nk:.6} {tx:.3} {ty:.3})" fill="#FFFFFF" d="{d}"/>
  <rect x="150" y="184" width="64" height="16" rx="8" fill="{YELLOW}"/>
</svg>
"##,
        nk = -k,
        d = path.0.trim(),
    ))
}

#[derive(Default)]
struct PathData(String);

impl ttf_parser::OutlineBuilder for PathData {
    fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "M{x} {y} ");
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "L{x} {y} ");
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(self.0, "Q{x1} {y1} {x} {y} ");
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(self.0, "C{x1} {y1} {x2} {y2} {x} {y} ");
    }
    fn close(&mut self) {
        self.0.push_str("Z ");
    }
}
