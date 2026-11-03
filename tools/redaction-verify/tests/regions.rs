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
const PAINTED_MESSAGE: &str = "painted geometry inside a redaction region";

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
// Masks.

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
    let covered = [DRAW, b"0 g 72 400 50 50 re f\n"].concat();
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
        let covered = [DRAW, b"0 g 72 400 50 50 re f\n"].concat();
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
// What the render sees and the images do not.

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
    // image check cannot place it, so it says so. The path that the pattern fills is painted
    // geometry under the region, which is a leak of its own.
    let covered = [
        paint.as_slice(),
        b"0 g 72 400 50 50 re f
",
    ]
    .concat();
    let r = cli(
        &plain_page(&covered, &[(16, rgb8(&orig)), (19, pattern)]),
        Some(REGION),
        &[],
        &[],
    );
    assert_eq!(r.code, 1, "{}", r.out);
    assert!(r.out.contains(PAINTED_MESSAGE), "{}", r.out);
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
    // Painted over by the fill: the render shows only the fill, and the path is still in the file.
    let covered = [
        sig.as_slice(),
        b"0 g 72 400 50 50 re f
",
    ]
    .concat();
    let black = region([72.0, 400.0, 122.0, 450.0], Some("[0,0,0]"));
    check(
        "covered by a fill",
        &plain_page(&covered, &[]),
        &black,
        Leak(&[PAINTED_MESSAGE, "fill of a path"], &[RENDER_MESSAGE]),
    );
    check(
        "covered, no fill named",
        &plain_page(&covered, &[]),
        REGION,
        Leak(&[PAINTED_MESSAGE], &[RENDER_MESSAGE]),
    );
    // The box alone is what a redaction paints.
    let boxed = plain_page(
        b"0 g 72 400 50 50 re f
",
        &[],
    );
    check("the box alone", &boxed, &black, Clean);
    check("the box alone, no fill named", &boxed, REGION, Clean);
    let red = region([72.0, 400.0, 122.0, 450.0], Some("[255,0,0]"));
    check(
        "covered by black, red named",
        &plain_page(&covered, &[]),
        &red,
        Leak(
            &[RENDER_MESSAGE, "not the fill colour", PAINTED_MESSAGE],
            &[],
        ),
    );
    check(
        "the box alone, black, red named",
        &boxed,
        &red,
        Leak(&[RENDER_MESSAGE, PAINTED_MESSAGE], &[]),
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
// A region that is too small to check.

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
// JPEG.

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

#[test]
fn an_image_drawn_by_the_glyphs_of_a_type3_font_is_said() {
    let names = vec!["a".to_owned()];
    let page = |glyph: &[u8]| {
        let mut fx = Fx::new(&[BENIGN, b"BT /F2 12 Tf 72 600 Td (A) Tj ET\n".as_slice()].concat());
        fx.extra.insert(6, fonts::type3_font(65, &names, 20, None));
        fx.extra.insert(20, stream_obj("", glyph, false));
        fx.build()
    };
    let with_image =
        page(b"1000 0 d0 q 500 0 0 500 0 0 cm BI /W 2 /H 2 /BPC 8 /CS /G ID abcd EI Q");
    let r = cli(&with_image, Some(REGION), &[], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(
        r.problems().iter().any(|p| p.contains("Type 3 font /F2")),
        "{}",
        r.out
    );
    // Glyphs that draw only a shape need no saying.
    let shape = page(b"500 0 d0 0 0 400 400 re f");
    let r = cli(&shape, Some(REGION), &[], &[]);
    assert_eq!(r.code, 0, "{}", r.out);
}

// ---------------------------------------------------------------------------------------------
// Painted geometry: paths, shadings and glyphs under a region are in the file whatever hides them.

const SIG: &[u8] = b"0 0 1 rg 75 405 m 90 440 l 100 410 l 115 445 l 118 405 l f\n";
const BOX: &[u8] = b"0 0 0 rg 72 400 50 50 re f\n";

fn black_region() -> String {
    region([72.0, 400.0, 122.0, 450.0], Some("[0,0,0]"))
}

fn white_region() -> String {
    region([72.0, 400.0, 122.0, 450.0], Some("[255,255,255]"))
}

/// A page with the benign text, this content, and these entries in its resources.
fn drawn(content: &[u8], resources: &str) -> Bytes {
    let mut fx = Fx::new(&[BENIGN, content].concat());
    fx.resources_extra.push_str(resources);
    fx.build()
}

#[test]
fn a_signature_under_the_box_is_a_leak() {
    let under = [SIG, BOX].concat();
    check(
        "under a black box, fill named",
        &plain_page(&under, &[]),
        &black_region(),
        Leak(&[PAINTED_MESSAGE, "fill of a path"], &[RENDER_MESSAGE]),
    );
    check(
        "under a black box, no fill named",
        &plain_page(&under, &[]),
        REGION,
        Leak(&[PAINTED_MESSAGE], &[RENDER_MESSAGE]),
    );
    // The box alone is what a redaction paints.
    check(
        "the box alone",
        &plain_page(BOX, &[]),
        &black_region(),
        Clean,
    );
    // A box that is not the region is geometry like any other: here it is the page.
    check(
        "a box that is the page",
        &plain_page(b"0 g 0 0 612 792 re f\n", &[]),
        &black_region(),
        Leak(&[PAINTED_MESSAGE], &[]),
    );
    // A box in the wrong colour is not the box that the region names.
    check(
        "a white box, black named",
        &plain_page(b"1 g 72 400 50 50 re f\n", &[]),
        &black_region(),
        Leak(&[PAINTED_MESSAGE, RENDER_MESSAGE], &[]),
    );
    // A stroke round the region is painted there too.
    check(
        "the box and its outline",
        &plain_page(b"0 g 72 400 50 50 re B\n", &[]),
        &black_region(),
        Leak(&[PAINTED_MESSAGE, "fill and stroke of a path"], &[]),
    );
}

#[test]
fn geometry_that_nothing_shows_is_still_there() {
    let alpha = " /ExtGState << /GS0 << /ca 0 /CA 0 >> >>";
    check(
        "alpha 0",
        &drawn(&[b"/GS0 gs ", SIG].concat(), alpha),
        &white_region(),
        Leak(&[PAINTED_MESSAGE], &[RENDER_MESSAGE]),
    );
    check(
        "clipped away",
        &drawn(&[b"q 300 300 10 10 re W n ", SIG, b"Q\n"].concat(), ""),
        &white_region(),
        Leak(&[PAINTED_MESSAGE], &[RENDER_MESSAGE]),
    );
    check(
        "hidden optional content",
        &Fx::new(BENIGN).hidden(SIG).build(),
        &white_region(),
        Leak(&[PAINTED_MESSAGE], &[RENDER_MESSAGE]),
    );
    let white_sig = String::from_utf8_lossy(SIG).replace("0 0 1 rg", "1 1 1 rg");
    check(
        "white on white",
        &drawn(white_sig.as_bytes(), ""),
        &white_region(),
        Leak(&[PAINTED_MESSAGE], &[RENDER_MESSAGE]),
    );
    // A blend mode that leaves the page as it was.
    check(
        "a blend mode that leaves the white",
        &drawn(
            &[b"/GS0 gs ", SIG].concat(),
            " /ExtGState << /GS0 << /BM /Lighten >> >>",
        ),
        &white_region(),
        Leak(&[PAINTED_MESSAGE], &[]),
    );
}

#[test]
fn geometry_that_is_not_under_the_region_is_not_a_leak() {
    check(
        "a signature elsewhere",
        &plain_page(b"0 0 1 rg 300 300 40 40 re f\n", &[]),
        &white_region(),
        Clean,
    );
    // A thin line above the region does not reach it: the stroke is widened by the line.
    check(
        "a line just above",
        &plain_page(b"0 0 1 RG 2 w 80 455 m 110 455 l S\n", &[]),
        &white_region(),
        Clean,
    );
    check(
        "a wide line that reaches in",
        &plain_page(b"0 0 1 RG 6 w 80 451 m 110 451 l S\n", &[]),
        &white_region(),
        Leak(&[PAINTED_MESSAGE, "stroke of a path"], &[]),
    );
    // A path that is built and not painted paints nothing.
    check(
        "a path that is ended with n",
        &plain_page(b"72 400 50 50 re W n\n", &[]),
        &white_region(),
        Clean,
    );
}

#[test]
fn geometry_in_a_form_follows_the_matrices() {
    let form = stream_obj(
        "/Type /XObject /Subtype /Form /BBox [0 0 50 50]",
        b"0 0 1 rg 3 5 40 40 re f",
        false,
    );
    let page = |at: &str| {
        let draw = format!("q 1 0 0 1 {at} cm /Fm1 Do Q\n");
        let mut fx = Fx::new(&[BENIGN, draw.as_bytes()].concat());
        fx.resources_extra.push_str(" /XObject << /Fm1 10 0 R >>");
        fx.extra.insert(10, form.clone());
        fx.build()
    };
    check(
        "a form drawn on the region",
        &page("72 400"),
        &white_region(),
        Leak(&[PAINTED_MESSAGE, "form object "], &[]),
    );
    check(
        "a form drawn elsewhere",
        &page("300 100"),
        &white_region(),
        Clean,
    );
}

#[test]
fn a_shading_is_painted_where_its_box_is() {
    let shading = |bbox: &str| {
        format!(
            " /Shading << /Sh1 << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 100 0] \
             /Function << /FunctionType 2 /Domain [0 1] /C0 [0 0 1] /C1 [1 0 0] /N 1 >> \
             /Extend [true true] {bbox}>> >>"
        )
    };
    check(
        "a shading with a box over the region",
        &drawn(b"/Sh1 sh\n", &shading("/BBox [72 400 122 450] ")),
        &white_region(),
        Leak(&[PAINTED_MESSAGE, "shading"], &[]),
    );
    check(
        "a shading with a box elsewhere",
        &drawn(b"/Sh1 sh\n", &shading("/BBox [300 300 400 400] ")),
        &white_region(),
        Clean,
    );
    // Without a box the shading paints the whole page.
    check(
        "a shading with no box",
        &drawn(b"/Sh1 sh\n", &shading("")),
        &white_region(),
        Leak(&[PAINTED_MESSAGE, "shading"], &[]),
    );
    // A shading that is not there: where it paints is not known.
    let r = cli(
        &drawn(b"/Nothing sh\n", ""),
        Some(&white_region()),
        &[],
        &[],
    );
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(
        r.problems().iter().any(|p| p.contains("shading /Nothing")),
        "{}",
        r.out
    );
}

#[test]
fn the_glyphs_of_a_type3_font_are_geometry_where_they_are_shown() {
    let names = vec!["a".to_owned()];
    let page = |text: &str| {
        let mut fx = Fx::new(&[BENIGN, text.as_bytes()].concat());
        fx.extra.insert(6, fonts::type3_font(65, &names, 20, None));
        fx.extra
            .insert(20, stream_obj("", b"1000 0 d0 0 0 800 800 re f", false));
        fx.build()
    };
    check(
        "shown on the region",
        &page("BT /F2 12 Tf 80 410 Td (A) Tj ET\n"),
        &white_region(),
        Leak(&[PAINTED_MESSAGE, "glyphs of a Type 3 font"], &[]),
    );
    // The position is the one after the moves of the text.
    check(
        "shown on the region after a move",
        &page("BT /F2 12 Tf 14 TL 80 560 Td 0 -150 Td (A) Tj ET\n"),
        &white_region(),
        Leak(&[PAINTED_MESSAGE, "glyphs of a Type 3 font"], &[]),
    );
    check(
        "shown elsewhere",
        &page("BT /F2 12 Tf 300 100 Td (A) Tj ET\n"),
        &white_region(),
        Clean,
    );
    check("never shown", &page(""), &white_region(), Clean);
    // A font whose glyphs have no box: where they paint is not known.
    let mut fx = Fx::new(&[BENIGN, b"BT /F2 12 Tf 300 100 Td (A) Tj ET\n".as_slice()].concat());
    fx.extra.insert(
        6,
        String::from_utf8_lossy(&fonts::type3_font(65, &names, 20, None))
            .replace("/FontBBox [0 0 1000 1000]", "/FontBBox [0 0 0 0]")
            .into_bytes(),
    );
    fx.extra
        .insert(20, stream_obj("", b"1000 0 d0 0 0 800 800 re f", false));
    let r = cli(&fx.build(), Some(&white_region()), &[], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(
        r.problems().iter().any(|p| p.contains("FontBBox")),
        "{}",
        r.out
    );
}

// ---------------------------------------------------------------------------------------------
// Every appearance that a viewer can draw.

const PAINTER: &str = "0 0 1 rg 3 5 40 40 re f";

fn form50(content: &str, resources: &str) -> Bytes {
    stream_obj(
        &format!("/Type /XObject /Subtype /Form /BBox [0 0 50 50] /Resources << {resources} >>"),
        content.as_bytes(),
        false,
    )
}

/// A page with a push button over the region. `ap` is the content of its `/AP` dictionary, `mk`
/// the entries of its `/MK`.
fn button_page(ap: &str, mk: &str, extra: &[(u32, Bytes)]) -> Bytes {
    let mut fx = Fx::new(BENIGN);
    fx.page_extra.push_str(" /Annots [30 0 R]");
    fx.extra.insert(
        30,
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (b1) /Rect [72 400 122 450] \
             /P 3 0 R /MK << {mk} >> /AP << {ap} >> >>"
        )
        .into_bytes(),
    );
    for (n, b) in extra {
        fx.extra.insert(*n, b.clone());
    }
    fx.build()
}

#[test]
fn every_state_of_every_appearance_is_walked_for_geometry() {
    let blank = (31, form50("q Q", ""));
    let painter = (32, form50(PAINTER, ""));
    let none = button_page("/N 31 0 R", "", std::slice::from_ref(&blank));
    check("a blank normal appearance", &none, &white_region(), Clean);
    for (name, ap, mk) in [
        ("the down appearance", "/N 31 0 R /D 32 0 R", ""),
        ("the rollover appearance", "/N 31 0 R /R 32 0 R", ""),
        (
            "a state of the normal appearance",
            "/N << /Off 31 0 R /Yes 32 0 R >>",
            "",
        ),
        (
            "a state of the down appearance",
            "/N 31 0 R /D << /Off 31 0 R /Yes 32 0 R >>",
            "",
        ),
        ("the icon of the button", "/N 31 0 R", "/I 32 0 R"),
        ("the rollover icon", "/N 31 0 R", "/RI 32 0 R"),
        ("the alternate icon", "/N 31 0 R", "/IX 32 0 R"),
    ] {
        check(
            name,
            &button_page(ap, mk, &[blank.clone(), painter.clone()]),
            &white_region(),
            Leak(&[PAINTED_MESSAGE, "an annotation appearance"], &[]),
        );
    }
}

#[test]
fn the_pixels_of_every_appearance_are_checked() {
    let image_form = |im: u32| {
        form50(
            "q 50 0 0 50 0 0 cm /Im Do Q",
            &format!("/XObject << /Im {im} 0 R >>"),
        )
    };
    let black = rgb8(&[0u8; 8 * 8 * 3]);
    let orig = rgb8(&original());
    let page = |down_image: &Bytes| {
        button_page(
            "/N 31 0 R /D 32 0 R",
            "",
            &[
                (16, down_image.clone()),
                (17, black.clone()),
                (31, image_form(17)),
                (32, image_form(16)),
            ],
        )
    };
    let spec = black_region();
    check(
        "the down appearance keeps the image",
        &page(&orig),
        &spec,
        Leak(&[IMAGE_MESSAGE], &[RENDER_MESSAGE]),
    );
    check("both appearances are blank", &page(&black), &spec, Clean);
}

// ---------------------------------------------------------------------------------------------
// JPEG that is turned, and a tolerance that comes from the tables that the image uses.

/// The matrix of a picture of 48 points turned by one degree about (100, 420).
fn turned() -> (f64, f64) {
    (
        1.0f64.to_radians().cos() * 48.0,
        1.0f64.to_radians().sin() * 48.0,
    )
}

/// A JPEG of 48 x 48 samples drawn 48 points wide, turned by one degree about (100, 420).
fn turned_jpeg_page(hex: &str, content_after: &[u8]) -> Bytes {
    let (c, s) = turned();
    let draw = format!("q {c:.4} {s:.4} {:.4} {c:.4} 100 420 cm /Im1 Do Q\n", -s);
    plain_page(
        &[draw.as_bytes(), content_after].concat(),
        &[(16, jpeg48(hex))],
    )
}

/// The page position of a point of the unit square of the turned image.
fn on_turned_page(u: f64, v: f64) -> (f64, f64) {
    let (c, s) = turned();
    (100.0 + c * u - s * v, 420.0 + s * u + c * v)
}

#[test]
fn a_jpeg_that_is_turned_is_checked_like_one_that_is_not() {
    // A region that covers the whole picture, with the box painted over it.
    let big = region([60.0, 400.0, 200.0, 500.0], Some("[0,0,0]"));
    let boxed = b"0 g 60 400 140 100 re f\n";
    check(
        "turned, not redacted",
        &turned_jpeg_page(Q75_420_ORIGINAL_HEX, boxed),
        &big,
        Leak(&[IMAGE_MESSAGE], &[RENDER_MESSAGE]),
    );
    // Without the colour of the fill, too.
    check(
        "turned, not redacted, no fill named",
        &turned_jpeg_page(Q75_420_ORIGINAL_HEX, boxed),
        &region([60.0, 400.0, 200.0, 500.0], None),
        Leak(&[IMAGE_MESSAGE], &[]),
    );
    // The black square of the redacted picture, turned with it, less a point at each edge.
    let corners = [(5.0, 11.0), (37.0, 11.0), (5.0, 43.0), (37.0, 43.0)]
        .map(|(col, v)| on_turned_page(col / 48.0, v / 48.0));
    let x0 = corners[0].0.max(corners[2].0) + 1.0;
    let x1 = corners[1].0.min(corners[3].0) - 1.0;
    let y0 = corners[0].1.max(corners[1].1) + 1.0;
    let y1 = corners[2].1.min(corners[3].1) - 1.0;
    let inside = region([x0, y0, x1, y1], Some("[0,0,0]"));
    check(
        "turned, redacted",
        &turned_jpeg_page(Q75_420_REDACTED_HEX, b""),
        &inside,
        Clean,
    );
    check(
        "turned, redacted, no fill named",
        &turned_jpeg_page(Q75_420_REDACTED_HEX, b""),
        &region([x0, y0, x1, y1], None),
        Clean,
    );
    check(
        "turned, wrong rows",
        &turned_jpeg_page(Q75_420_WRONG_ROWS_HEX, b""),
        &inside,
        Leak(&[IMAGE_MESSAGE, RENDER_MESSAGE], &[]),
    );
}

#[test]
fn a_jpeg_that_no_whole_block_lies_in_is_could_not_check_even_with_the_fill_named() {
    // Ten points of black inside the black square: less than one block.
    let fill = Some("[0,0,0]");
    for (name, bytes, rect) in [
        (
            "not turned",
            jpeg_page(Q75_420_REDACTED_HEX),
            [85.0, 420.0, 95.0, 430.0],
        ),
        (
            "turned",
            turned_jpeg_page(Q75_420_REDACTED_HEX, b""),
            [110.0, 440.0, 120.0, 450.0],
        ),
    ] {
        let r = cli(&bytes, Some(&region(rect, fill)), &[], &[]);
        assert_eq!(
            r.code, 2,
            "{name}
{}",
            r.out
        );
        assert!(
            r.problems()
                .iter()
                .any(|p| p.contains("no whole block of it")),
            "{name}
{}",
            r.out
        );
        assert!(
            !r.out.contains("LEAK"),
            "{name}
{}",
            r.out
        );
    }
}

/// The JPEG with one more quantisation table (number 3) right after the start marker. No
/// component uses it.
fn with_unused_table(jpeg: &[u8], wide: bool, value: u16) -> Vec<u8> {
    let mut seg = vec![if wide { 0x13 } else { 0x03 }];
    for _ in 0..64 {
        if wide {
            seg.extend(value.to_be_bytes());
        } else {
            seg.push(value as u8);
        }
    }
    let mut out = jpeg[..2].to_vec();
    out.extend([0xFF, 0xDB]);
    out.extend(u16::try_from(seg.len() + 2).unwrap().to_be_bytes());
    out.extend(seg);
    out.extend(&jpeg[2..]);
    out
}

fn jpeg_page_of(bytes: &[u8]) -> Bytes {
    plain_page(
        b"q 48 0 0 48 72 400 cm /Im1 Do Q\n",
        &[(
            16,
            stream_obj(
                "/Type /XObject /Subtype /Image /Width 48 /Height 48 /ColorSpace /DeviceRGB \
                 /BitsPerComponent 8 /Filter /DCTDecode",
                bytes,
                false,
            ),
        )],
    )
}

#[test]
fn a_table_that_no_component_uses_does_not_loosen_the_check() {
    let spec = region(JPEG_REGION, Some("[0,0,0]"));
    for (name, wide, value) in [("8-bit", false, 255), ("16-bit", true, 65535)] {
        check(
            &format!("not redacted, {name} table that nothing uses"),
            &jpeg_page_of(&with_unused_table(
                &unhex(Q75_420_ORIGINAL_HEX),
                wide,
                value,
            )),
            &spec,
            Leak(&[IMAGE_MESSAGE, RENDER_MESSAGE], &[]),
        );
        check(
            &format!("redacted, {name} table that nothing uses"),
            &jpeg_page_of(&with_unused_table(
                &unhex(Q75_420_REDACTED_HEX),
                wide,
                value,
            )),
            &spec,
            Clean,
        );
    }
}

fn gray48_page(hex: &str) -> Bytes {
    plain_page(
        b"q 48 0 0 48 72 400 cm /Im1 Do Q\n",
        &[(
            16,
            stream_obj(
                "/Type /XObject /Subtype /Image /Width 48 /Height 48 /ColorSpace /DeviceGray \
                 /BitsPerComponent 8 /Filter /DCTDecode",
                &unhex(hex),
                false,
            ),
        )],
    )
}

#[test]
fn a_jpeg_with_coarse_tables_is_could_not_check() {
    let spec = region(JPEG_REGION, Some("[0,0,0]"));
    // Quality 10 is the coarsest that the check accepts.
    check("quality 10", &gray48_page(BLACK_48_Q10_HEX), &spec, Clean);
    let r = cli(&gray48_page(BLACK_48_Q5_HEX), Some(&spec), &[], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(
        r.problems().iter().any(|p| p.contains("so coarse")),
        "{}",
        r.out
    );
}

#[test]
fn the_tolerance_of_a_jpeg_is_for_its_own_pixels_only() {
    // A white JPEG with a table of 255 that nothing uses, and light gray text in the region, not
    // over the picture. The text is no geometry: only the render sees it.
    let jpeg = with_unused_table(&unhex(WHITE_16_HEX), false, 255);
    let page = |text: &[u8]| {
        let mut fx = Fx::new(&[BENIGN, b"q 16 0 0 16 72 400 cm /Im1 Do Q\n", text].concat());
        fx.resources_extra.push_str(" /XObject << /Im1 16 0 R >>");
        fx.extra.insert(
            16,
            stream_obj(
                "/Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceGray \
                 /BitsPerComponent 8 /Filter /DCTDecode",
                &jpeg,
                false,
            ),
        );
        fx.build()
    };
    let spec = region([72.0, 400.0, 172.0, 500.0], Some("[255,255,255]"));
    check("the white picture alone", &page(b""), &spec, Clean);
    check(
        "light gray text beside it",
        &page(b"0.9 g BT /F1 24 Tf 110 460 Td (X) Tj ET\n"),
        &spec,
        Leak(&[RENDER_MESSAGE, "not the fill colour"], &[PAINTED_MESSAGE]),
    );
}

#[test]
fn an_annotation_that_a_viewer_draws_from_its_own_entries_is_could_not_check() {
    let page = |rect: &str| {
        let mut fx = Fx::new(BENIGN);
        fx.page_extra.push_str(" /Annots [30 0 R]");
        fx.extra.insert(
            30,
            format!(
                "<< /Type /Annot /Subtype /Ink /Rect [{rect}] /InkList [[75 405 90 440 100 410]]                  /C [0 0 1] /F 2 >>"
            )
            .into_bytes(),
        );
        fx.build()
    };
    // The annotation is hidden, so the render shows nothing; a viewer can show it again.
    let r = cli(&page("72 400 122 450"), Some(&white_region()), &[], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(
        r.problems()
            .iter()
            .any(|p| p.contains("Ink annotation with no appearance stream")),
        "{}",
        r.out
    );
    // Elsewhere on the page it is no business of the region.
    check(
        "an ink annotation elsewhere",
        &page("300 300 350 350"),
        &white_region(),
        Clean,
    );
}
