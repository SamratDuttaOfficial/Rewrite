//! What the GTK views use: page rendering, text in a rectangle, the version cache, and calls from several
//! threads at once.

use rewrite_pdfium::{first_page_png, Cached, Document};
use rewrite_testkit::root;
use std::sync::Arc;

fn pdf(name: &str) -> Vec<u8> {
    std::fs::read(root().join("tests").join("corpus").join("pdf").join(name)).expect("corpus PDF")
}

#[test]
fn render_page_size_and_pixels() {
    let doc = Document::open(pdf("document.pdf")).unwrap();
    assert_eq!(doc.page_count(), 2);
    let bm = doc.render_page(0, 1.5).unwrap();
    // A4 is 595.28 x 841.89 points.
    assert_eq!((bm.width, bm.height), (893, 1263));
    assert_eq!(bm.pixels.len(), bm.width * bm.height * 4);
    let px = |x: usize, y: usize| &bm.pixels[(y * bm.width + x) * 4..][..4];
    assert_eq!(px(5, 5), [255, 255, 255, 255]);
    // The title's "E" (about 196..211 x 80..98 points from the top-left) is drawn in dark ink.
    let dark = (120..146).flat_map(|y| (294..316).map(move |x| (x, y))).filter(|&(x, y)| px(x, y)[0] < 100).count();
    assert!(dark > 30, "only {dark} dark pixels in the title");
    // The same pixels as the BGR rendering used for conversion, but for blending rounded the other way.
    let png = first_page_png(&pdf("document.pdf"), 1.5).unwrap();
    let rgb = rewrite_core::imaging::decode(&png).unwrap().to_rgb8();
    assert_eq!((rgb.width() as usize, rgb.height() as usize), (bm.width, bm.height));
    assert!(rgb.pixels().zip(bm.pixels.chunks_exact(4)).all(|(a, b)| (0..3).all(|i| a.0[i].abs_diff(b[i]) <= 1)));
}

#[test]
fn render_rotated_page() {
    let doc = Document::open(pdf("rotated.pdf")).unwrap();
    let bm = doc.render_page(0, 1.0).unwrap();
    assert_eq!((bm.width, bm.height), (842, 596));
    assert_eq!(doc.page_sizes().unwrap().len(), 2);
    assert!(doc.render_page(2, 1.0).is_err());
}

#[test]
fn text_bounded() {
    let doc = Document::open(pdf("document.pdf")).unwrap();
    let h = 841.89;
    let title = doc.text_bounded(0, 190.0, h - 100.0, 420.0, h - 75.0).unwrap();
    assert_eq!(title.trim(), "Efficient Training");
    assert_eq!(doc.text_bounded(0, 5.0, 5.0, 20.0, 20.0).unwrap(), "");
    let chars = doc.chars(0, 0.0, h).unwrap();
    assert_eq!(chars.iter().take(18).map(|c| c.text.as_str()).collect::<String>(), "Efficient Training");
    assert!(chars[0].font.contains("Georgia") && chars[0].size > 20.0);
}

#[test]
fn cached_reopens_on_new_version() {
    let mut cache = Cached::default();
    let a: Arc<[u8]> = pdf("document.pdf").into();
    let b: Arc<[u8]> = pdf("sample.pdf").into();
    assert_eq!(cache.get(1, || a.clone()).unwrap().page_count(), 2);
    assert_eq!(cache.get(1, || panic!("not reopened")).unwrap().page_count(), 2);
    assert_eq!(cache.get(2, || b.clone()).unwrap().page_count(), 1);
    assert!(cache.get(3, || Arc::from(&b"junk"[..])).is_err());
}

#[test]
fn many_threads() {
    let doc = Arc::new(Document::open(pdf("sample.pdf")).unwrap());
    let threads: Vec<_> = (0..6)
        .map(|i| {
            let doc = doc.clone();
            std::thread::spawn(move || {
                let bm = doc.render_page(0, 0.5 + i as f64 * 0.1).unwrap();
                let pages = doc.read_pages(i % 2 == 0, None, None).unwrap();
                (bm.width, pages[0].chars.len())
            })
        })
        .collect();
    let got: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(got.iter().all(|g| g.1 == got[0].1 && g.1 > 100));
}
