//! Redacted regions. A region passes when the page, rendered by Poppler, is the fill colour inside
//! it (less an edge of two pixels), and the images under it, with their masks, are one value. Each
//! case is a small file; the tests say which of the two checks sees it, because each must see what
//! the other cannot: the render sees masks, patterns and vector shapes, and the image check sees
//! pixels that something else was painted over.

mod common;

use common::hard_cases::*;
use common::jpeg::*;
use common::*;

const IMAGE_MESSAGE: &str = "inside a redaction region is not blank";
const RENDER_MESSAGE: &str = "rendered pixels inside a redaction region";

/// 8 x 8 RGB samples that differ from each other everywhere.
fn original() -> Vec<u8> {
    let mut px = Vec::new();
    for y in 0..8u32 {
        for x in 0..8u32 {
            px.extend([
                ((x * 31 + y * 7) % 256) as u8,
                ((y * 29 + 50) % 256) as u8,
                ((x * y * 13) % 256) as u8,
            ]);
        }
    }
    px
}

/// The samples with these rows and columns set to black.
fn blanked(px: &[u8], rows: std::ops::Range<usize>, cols: std::ops::Range<usize>) -> Vec<u8> {
    let mut px = px.to_vec();
    for y in rows {
        for x in cols.clone() {
            px[(y * 8 + x) * 3..][..3].copy_from_slice(&[0, 0, 0]);
        }
    }
    px
}

fn rgb8(px: &[u8]) -> Bytes {
    Fx::image(8, 8, 3, px)
}

/// The image (object 16) drawn over 100 x 100 points at (72, 400): the lower left quarter of the
/// region below is rows 4 to 7 and columns 0 to 3.
const DRAW: &[u8] = b"q 100 0 0 100 72 400 cm /Im1 Do Q\n";
const REGION: &str = r#"{"regions":[{"page":1,"rect":[72,400,122,450]}]}"#;

/// A page with this content, media box and turn, an image resource, and these objects.
fn page(content: &[u8], media: &str, rotate: i32, extra: &[(u32, Bytes)]) -> Bytes {
    let mut fx = Fx::new(&[BENIGN, content].concat());
    fx.extra.insert(
        3,
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox {media} /Rotate {rotate} /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R /F2 6 0 R >> /XObject << /Im1 16 0 R /Im2 17 0 R >> \
             /Pattern << /P1 19 0 R >> >> >>"
        )
        .into_bytes(),
    );
    for (n, b) in extra {
        fx.extra.insert(*n, b.clone());
    }
    fx.build()
}

fn plain_page(content: &[u8], extra: &[(u32, Bytes)]) -> Bytes {
    page(content, "[0 0 612 792]", 0, extra)
}

fn image_page(px: &[u8]) -> Bytes {
    plain_page(DRAW, &[(16, rgb8(px))])
}

fn region(rect: [f64; 4], fill: Option<&str>) -> String {
    let fill = fill.map_or(String::new(), |f| format!(r#","fill":{f}"#));
    format!(
        r#"{{"regions":[{{"page":1,"rect":[{},{},{},{}]{fill}}}]}}"#,
        rect[0], rect[1], rect[2], rect[3]
    )
}

#[derive(Debug)]
enum Expect {
    Clean,
    /// A leak, with the messages that must be in the report and those that must not.
    Leak(&'static [&'static str], &'static [&'static str]),
}
use Expect::*;

#[track_caller]
fn check(name: &str, bytes: &[u8], spec: &str, expect: Expect) {
    let r = cli(bytes, Some(spec), &[], &[]);
    match expect {
        Clean => {
            assert_eq!(r.code, 0, "{name}: not clean\n{}", r.out);
            assert!(r.out.contains("clean"), "{name}\n{}", r.out);
        }
        Leak(with, without) => {
            assert_eq!(r.code, 1, "{name}: not a leak\n{}", r.out);
            assert_eq!(r.layers().len(), 1, "{name}\n{}", r.out);
            assert!(r.layers().contains("images"), "{name}\n{}", r.out);
            for m in with {
                assert!(r.out.contains(m), "{name}: no \"{m}\"\n{}", r.out);
            }
            for m in without {
                assert!(!r.out.contains(m), "{name}: \"{m}\" is there\n{}", r.out);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Images, as before.

#[test]
fn an_image_that_is_not_redacted_is_a_leak() {
    let orig = original();
    check(
        "unredacted",
        &image_page(&orig),
        REGION,
        Leak(&[IMAGE_MESSAGE, RENDER_MESSAGE], &[]),
    );
    check(
        "wrong rows blanked",
        &image_page(&blanked(&orig, 0..4, 0..4)),
        REGION,
        Leak(&[IMAGE_MESSAGE, RENDER_MESSAGE], &[]),
    );
    check(
        "redacted",
        &image_page(&blanked(&orig, 4..8, 0..4)),
        REGION,
        Clean,
    );
    // With the colour the redaction was painted with.
    check(
        "redacted, fill named",
        &image_page(&blanked(&orig, 4..8, 0..4)),
        &region([72.0, 400.0, 122.0, 450.0], Some("[0,0,0]")),
        Clean,
    );
    check(
        "redacted black, fill named white",
        &image_page(&blanked(&orig, 4..8, 0..4)),
        &region([72.0, 400.0, 122.0, 450.0], Some(r##""#FFFFFF""##)),
        Leak(&[RENDER_MESSAGE, "not the fill colour"], &[IMAGE_MESSAGE]),
    );
}

#[test]
fn the_page_geometry_is_followed() {
    let orig = original();
    let ok = blanked(&orig, 4..8, 0..4);
    for rotate in [0, 90, 180, 270] {
        let build = |px: &[u8]| page(DRAW, "[0 0 612 792]", rotate, &[(16, rgb8(px))]);
        check(
            &format!("turned {rotate}, unredacted"),
            &build(&orig),
            REGION,
            Leak(&[IMAGE_MESSAGE, RENDER_MESSAGE], &[]),
        );
        check(
            &format!("turned {rotate}, redacted"),
            &build(&ok),
            REGION,
            Clean,
        );
    }
    // A media box that does not start at the origin: user space is not the render.
    let mb = "[100 100 712 892]";
    let draw = b"q 100 0 0 100 172 500 cm /Im1 Do Q\n";
    let spec = region([172.0, 500.0, 222.0, 550.0], None);
    check(
        "media box, unredacted",
        &page(draw, mb, 0, &[(16, rgb8(&orig))]),
        &spec,
        Leak(&[IMAGE_MESSAGE, RENDER_MESSAGE], &[]),
    );
    check(
        "media box, redacted",
        &page(draw, mb, 0, &[(16, rgb8(&ok))]),
        &spec,
        Clean,
    );
    // The region in the coordinates of a page that starts at the origin is another place, where
    // nothing was painted: with the fill named, that is seen.
    check(
        "media box, region in the wrong coordinates",
        &page(draw, mb, 0, &[(16, rgb8(&orig))]),
        &region([72.0, 400.0, 122.0, 450.0], Some("[0,0,0]")),
        Leak(&["not the fill colour"], &[]),
    );
    // Upside down: the rows that the region covers are the first ones.
    let flip = b"q 100 0 0 -100 72 500 cm /Im1 Do Q\n";
    check(
        "flipped, the rows of an upright image blanked",
        &plain_page(flip, &[(16, rgb8(&ok))]),
        REGION,
        Leak(&[IMAGE_MESSAGE, RENDER_MESSAGE], &[]),
    );
    check(
        "flipped, the rows of a flipped image blanked",
        &plain_page(flip, &[(16, rgb8(&blanked(&orig, 0..4, 0..4)))]),
        REGION,
        Clean,
    );
    // One image drawn twice, once under the region; and a blanked copy under it.
    let twice = b"q 100 0 0 100 300 400 cm /Im1 Do Q\nq 100 0 0 100 72 400 cm /Im1 Do Q\n";
    check(
        "drawn twice",
        &plain_page(twice, &[(16, rgb8(&orig))]),
        REGION,
        Leak(&[IMAGE_MESSAGE, RENDER_MESSAGE], &[]),
    );
    let copy = b"q 100 0 0 100 300 400 cm /Im1 Do Q\nq 100 0 0 100 72 400 cm /Im2 Do Q\n";
    check(
        "one copy blanked under the region",
        &plain_page(copy, &[(16, rgb8(&orig)), (17, rgb8(&ok))]),
        REGION,
        Clean,
    );
}

// ---------------------------------------------------------------------------------------------
// C4: masks.

/// A gray 8 x 8 mask with a shape in it.
fn shape() -> Vec<u8> {
    (0..64u32).map(|i| ((i * 37 + 11) % 256) as u8).collect()
}

fn gray8(samples: &[u8]) -> Bytes {
    Fx::image(8, 8, 1, samples)
}

/// An 8 x 8 image of black samples with a soft mask (object 18).
fn soft_masked_image() -> Bytes {
    stream_obj(
        "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceRGB \
         /BitsPerComponent 8 /SMask 18 0 R",
        &[0u8; 8 * 8 * 3],
        true,
    )
}

fn with_soft_mask(mask: &[u8]) -> Bytes {
    plain_page(DRAW, &[(16, soft_masked_image()), (18, gray8(mask))])
}

#[test]
fn a_shape_that_is_only_in_the_soft_mask_is_a_leak() {
    // The colour samples are one colour; the whole picture is in the mask.
    check(
        "shape in the soft mask",
        &with_soft_mask(&shape()),
        REGION,
        Leak(&["its soft mask", RENDER_MESSAGE], &[]),
    );
    // The engine blanked the colour samples and kept the mask: this is that file.
    let mut mask_bottom_kept = vec![255u8; 64];
    for (i, m) in mask_bottom_kept.iter_mut().enumerate().skip(32) {
        *m = shape()[i];
    }
    check(
        "mask kept under the region",
        &with_soft_mask(&mask_bottom_kept),
        REGION,
        Leak(&["its soft mask"], &[]),
    );
    // A mask that is one value is nothing to see, whatever the value.
    check("clear mask", &with_soft_mask(&[0u8; 64]), REGION, Clean);
    check("opaque mask", &with_soft_mask(&[255u8; 64]), REGION, Clean);
}

#[test]
fn the_soft_mask_alone_is_seen_when_the_render_is_not_there() {
    // Only the image check has a mask message; with the image drawn and covered by a fill painted
    // over it, the render is clean and the mask still shows.
    let covered = [DRAW, b"0 g 72 400 100 100 re f\n"].concat();
    let bytes = plain_page(
        &covered,
        &[(16, soft_masked_image()), (18, gray8(&shape()))],
    );
    check(
        "covered by a fill",
        &bytes,
        REGION,
        Leak(&["its soft mask"], &[RENDER_MESSAGE]),
    );
}

#[test]
fn a_stencil_mask_is_checked_too() {
    let black = vec![0u8; 8 * 8 * 3];
    let image = |mask: &[u8]| {
        let img = stream_obj(
            "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceRGB \
             /BitsPerComponent 8 /Mask 18 0 R",
            &black,
            true,
        );
        let stencil = stream_obj(
            "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ImageMask true \
             /BitsPerComponent 1",
            mask,
            true,
        );
        let covered = [DRAW, b"0 g 72 400 100 100 re f\n"].concat();
        plain_page(&covered, &[(16, img), (18, stencil)])
    };
    // One byte is one row of eight samples.
    check(
        "stencil with a shape",
        &image(&[0xAA, 0x55, 0xF0, 0x0F, 0xAA, 0x55, 0xF0, 0x0F]),
        REGION,
        Leak(&["its mask"], &[RENDER_MESSAGE]),
    );
    check("stencil of one value", &image(&[0xFF; 8]), REGION, Clean);
}

// ---------------------------------------------------------------------------------------------
// I9: what the render sees and the images do not.

#[test]
fn an_image_drawn_through_a_pattern_is_seen_by_the_render() {
    let pattern = stream_obj(
        "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 100 100] \
         /XStep 100 /YStep 100 /Resources << /XObject << /Im1 16 0 R >> >> \
         /Matrix [1 0 0 1 72 400]",
        b"q 100 0 0 100 0 0 cm /Im1 Do Q",
        true,
    );
    let orig = original();
    let paint = b"/Pattern cs /P1 scn 72 400 100 100 re f\n";
    check(
        "pattern",
        &plain_page(paint, &[(16, rgb8(&orig)), (19, pattern.clone())]),
        REGION,
        Leak(&[RENDER_MESSAGE], &[IMAGE_MESSAGE]),
    );
    // Painted over, the pixels are gone from the page and the image is still in the file: the
    // image check cannot place it, so it says so.
    let covered = [paint.as_slice(), b"0 g 72 400 100 100 re f\n"].concat();
    let r = cli(
        &plain_page(&covered, &[(16, rgb8(&orig)), (19, pattern)]),
        Some(REGION),
        &[],
        &[],
    );
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(
        r.problems()
            .iter()
            .any(|p| p.contains("tiling pattern /P1")),
        "{}",
        r.out
    );
}

#[test]
fn a_soft_mask_group_with_an_image_is_followed() {
    let group = stream_obj(
        "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency >> \
         /Resources << /XObject << /Im1 16 0 R >> >>",
        DRAW,
        true,
    );
    let orig = original();
    // The graphics state names the group, and the page resources need the state.
    let mut fx = Fx::new(&[BENIGN, b"/GS1 gs 0 g 72 400 100 100 re f\n".as_slice()].concat());
    fx.resources_extra
        .push_str(" /ExtGState << /GS1 21 0 R >> /XObject << /Im1 16 0 R >>");
    fx.extra.insert(16, rgb8(&orig));
    fx.extra.insert(20, group);
    fx.extra.insert(
        21,
        b"<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 20 0 R >> >>".to_vec(),
    );
    check(
        "image in a soft mask group",
        &fx.build(),
        REGION,
        Leak(&[IMAGE_MESSAGE], &[]),
    );
}

#[test]
fn vector_shapes_are_seen_by_the_render() {
    let sig = b"0 0 1 rg 75 405 m 90 440 l 100 410 l 115 445 l 118 405 l f\n";
    check(
        "a signature drawn as a path",
        &plain_page(sig, &[]),
        REGION,
        Leak(&[RENDER_MESSAGE], &[IMAGE_MESSAGE]),
    );
    // Painted over by the fill.
    let covered = [sig.as_slice(), b"0 g 72 400 50 50 re f\n"].concat();
    let black = region([72.0, 400.0, 122.0, 450.0], Some("[0,0,0]"));
    check(
        "covered by a fill",
        &plain_page(&covered, &[]),
        &black,
        Clean,
    );
    check(
        "covered, no fill named",
        &plain_page(&covered, &[]),
        REGION,
        Clean,
    );
    let red = region([72.0, 400.0, 122.0, 450.0], Some("[255,0,0]"));
    check(
        "covered by black, red named",
        &plain_page(&covered, &[]),
        &red,
        Leak(&[RENDER_MESSAGE, "not the fill colour"], &[]),
    );
    // Text turned into outlines is a shape like any other.
    check(
        "nothing painted, a fill named",
        &plain_page(b"", &[]),
        &black,
        Leak(&["not the fill colour"], &[]),
    );
    check("nothing painted", &plain_page(b"", &[]), REGION, Clean);
}

#[test]
fn only_the_edge_of_a_fill_is_forgiven() {
    let fill = b"0 g 72 400 100 100 re f\n";
    let page = plain_page(fill, &[]);
    let spec = |grow: f64| {
        region(
            [72.0 - grow, 400.0 - grow, 172.0 + grow, 500.0 + grow],
            Some("[0,0,0]"),
        )
    };
    check("the region is the fill", &page, &spec(0.0), Clean);
    check("half a point more", &page, &spec(0.5), Clean);
    check(
        "five points more",
        &page,
        &spec(5.0),
        Leak(&["not the fill colour"], &[]),
    );
    // The same on a page that is turned, where a mistake in the mapping moves the fill.
    for rotate in [90, 180, 270] {
        let turned = page_with_rotate(fill, rotate);
        check(&format!("turned {rotate}"), &turned, &spec(0.0), Clean);
        let elsewhere = region([300.0, 600.0, 400.0, 700.0], Some("[0,0,0]"));
        check(
            &format!("turned {rotate}, another place"),
            &turned,
            &elsewhere,
            Leak(&["not the fill colour"], &[]),
        );
    }
}

fn page_with_rotate(content: &[u8], rotate: i32) -> Bytes {
    page(content, "[0 0 612 792]", rotate, &[])
}

// ---------------------------------------------------------------------------------------------
// m8: a region that is too small to check.

#[test]
fn a_region_with_no_pixel_left_to_check_is_could_not_check() {
    let page = image_page(&original());
    for rect in [
        [72.0, 400.0, 74.0, 402.0],
        [72.0, 400.0, 150.0, 401.0],
        [72.0, 400.0, 72.0, 450.0],
    ] {
        let r = cli(&page, Some(&region(rect, None)), &[], &[]);
        assert_eq!(r.code, 2, "{rect:?}\n{}", r.out);
        assert!(
            r.problems().iter().any(|p| p.contains("is smaller than")),
            "{rect:?}\n{}",
            r.out
        );
        assert!(!r.out.contains("clean"), "{}", r.out);
    }
    // A region outside the page.
    let r = cli(
        &page,
        Some(&region([700.0, 900.0, 800.0, 950.0], None)),
        &[],
        &[],
    );
    assert_eq!(r.code, 2, "{}", r.out);
    // A region of one image sample, or less than one, is one value whether or not the image was
    // redacted: without the fill colour that is "could not check". With it, the render tells.
    // (A region off the grid of the samples that holds one whole sample shows several colours in
    // the render: that is a leak with or without the fill.)
    check(
        "one whole sample off the grid",
        &page,
        &region([78.0, 406.0, 103.0, 431.0], None),
        Leak(&[RENDER_MESSAGE, "of one colour"], &[]),
    );
    for rect in [[72.0, 400.0, 84.5, 412.5], [75.0, 403.0, 80.0, 408.0]] {
        let r = cli(&page, Some(&region(rect, None)), &[], &[]);
        assert_eq!(r.code, 2, "{rect:?}\n{}", r.out);
        assert!(
            r.problems()
                .iter()
                .any(|p| p.contains("whole sample(s) of the image")),
            "{rect:?}\n{}",
            r.out
        );
        check(
            &format!("{rect:?}, fill named"),
            &page,
            &region(rect, Some("[0,0,0]")),
            Leak(&[RENDER_MESSAGE], &[]),
        );
    }
    // The same small region over a sample that was blanked is clean when the fill is named.
    let blank = image_page(&blanked(&original(), 7..8, 0..1));
    check(
        "one sample, blanked",
        &blank,
        &region([72.0, 400.0, 84.5, 412.5], Some("[0,0,0]")),
        Clean,
    );
}

// ---------------------------------------------------------------------------------------------
// I10: JPEG.

fn jpeg48(hex: &str) -> Bytes {
    stream_obj(
        "/Type /XObject /Subtype /Image /Width 48 /Height 48 /ColorSpace /DeviceRGB \
         /BitsPerComponent 8 /Filter /DCTDecode",
        &unhex(hex),
        false,
    )
}

fn jpeg_page(hex: &str) -> Bytes {
    plain_page(b"q 48 0 0 48 72 400 cm /Im1 Do Q\n", &[(16, jpeg48(hex))])
}

/// The samples 5 to 36 of the picture, in points.
const JPEG_REGION: [f64; 4] = [77.0, 411.0, 109.0, 443.0];

#[test]
fn a_jpeg_that_was_redacted_and_encoded_again_passes() {
    let spec = region(JPEG_REGION, None);
    for (name, hex) in [
        ("quality 75, 4:2:0", Q75_420_REDACTED_HEX),
        ("quality 90, 4:2:0", Q90_420_REDACTED_HEX),
        ("quality 75, 4:4:4", Q75_444_REDACTED_HEX),
    ] {
        check(name, &jpeg_page(hex), &spec, Clean);
    }
    // With the colour that was painted, and with the tolerance of a coarse table.
    check(
        "fill named",
        &jpeg_page(Q75_420_REDACTED_HEX),
        &region(JPEG_REGION, Some("[0,0,0]")),
        Clean,
    );
}

#[test]
fn a_jpeg_that_was_not_redacted_or_redacted_in_the_wrong_rows_fails() {
    let spec = region(JPEG_REGION, None);
    for (name, hex) in [
        ("quality 75, original", Q75_420_ORIGINAL_HEX),
        ("quality 90, original", Q90_420_ORIGINAL_HEX),
        ("quality 75, wrong rows", Q75_420_WRONG_ROWS_HEX),
        ("quality 90, wrong rows", Q90_420_WRONG_ROWS_HEX),
    ] {
        check(
            name,
            &jpeg_page(hex),
            &spec,
            Leak(&[IMAGE_MESSAGE, RENDER_MESSAGE], &[]),
        );
    }
}

#[test]
fn a_jpeg_is_compared_with_the_fill_that_the_region_names() {
    // The picture was blanked to black; the region says it was filled with white.
    check(
        "black, white named",
        &jpeg_page(Q75_420_REDACTED_HEX),
        &region(JPEG_REGION, Some("[255,255,255]")),
        Leak(&[IMAGE_MESSAGE, RENDER_MESSAGE], &[]),
    );
    // A gray fill that is close to the black of the picture, within what the table allows, is not
    // a leak; one that is far from it is.
    check(
        "black, a dark gray named",
        &jpeg_page(Q75_420_REDACTED_HEX),
        &region(JPEG_REGION, Some("[2,2,2]")),
        Clean,
    );
    check(
        "black, a mid gray named",
        &jpeg_page(Q75_420_REDACTED_HEX),
        &region(JPEG_REGION, Some("[128,128,128]")),
        Leak(&[IMAGE_MESSAGE, RENDER_MESSAGE], &[]),
    );
}

#[test]
fn a_jpeg_region_with_no_whole_block_is_could_not_check() {
    // Ten samples across: no block of 16 lies inside it.
    let spec = region([77.0, 411.0, 87.0, 421.0], None);
    let r = cli(&jpeg_page(Q75_420_REDACTED_HEX), Some(&spec), &[], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(
        r.problems().iter().any(|p| p.contains("straddle")),
        "{}",
        r.out
    );
}

#[test]
fn the_soft_mask_of_a_jpeg_is_checked() {
    let image = |mask: &[u8]| {
        let jpeg = stream_obj(
            "/Type /XObject /Subtype /Image /Width 48 /Height 48 /ColorSpace /DeviceRGB \
             /BitsPerComponent 8 /Filter /DCTDecode /SMask 18 0 R",
            &unhex(Q75_420_REDACTED_HEX),
            false,
        );
        let mask = stream_obj(
            "/Type /XObject /Subtype /Image /Width 48 /Height 48 /ColorSpace /DeviceGray \
             /BitsPerComponent 8",
            mask,
            true,
        );
        plain_page(
            b"q 48 0 0 48 72 400 cm /Im1 Do Q\n",
            &[(16, jpeg), (18, mask)],
        )
    };
    let varied: Vec<u8> = (0..48 * 48u32)
        .map(|i| ((i * 7 + i / 48 * 13) % 256) as u8)
        .collect();
    check(
        "soft mask kept",
        &image(&varied),
        &region(JPEG_REGION, None),
        Leak(&["its soft mask"], &[]),
    );
    check(
        "soft mask of one value",
        &image(&[255u8; 48 * 48]),
        &region(JPEG_REGION, None),
        Clean,
    );
}
