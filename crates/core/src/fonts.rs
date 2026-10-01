//! A stand-in for the fonts of Shopify's font library, which are not available offline.
//!
//! Themes load their fonts from Shopify's CDN (`font_face`, `font_url`). Locally those URLs
//! answer with this font unless the real file was dropped into `files/fonts/`. It is a valid
//! TrueType font that maps no character at all, so browsers load it without errors and render
//! every character with the next family of the `font-family` list: the fallback the theme
//! declares.

fn u16be(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn i16be(out: &mut Vec<u8>, value: i16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn u32be(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

fn head() -> Vec<u8> {
    let mut t = Vec::with_capacity(54);
    u32be(&mut t, 0x0001_0000); // version
    u32be(&mut t, 0x0001_0000); // fontRevision
    u32be(&mut t, 0); // checkSumAdjustment, patched once the font is assembled
    u32be(&mut t, 0x5F0F_3CF5); // magicNumber
    u16be(&mut t, 0x000B); // flags
    u16be(&mut t, 1000); // unitsPerEm
    t.extend_from_slice(&[0; 16]); // created, modified
    for value in [0, 0, 10, 10] {
        i16be(&mut t, value); // xMin, yMin, xMax, yMax
    }
    u16be(&mut t, 0); // macStyle
    u16be(&mut t, 8); // lowestRecPPEM
    i16be(&mut t, 2); // fontDirectionHint
    i16be(&mut t, 0); // indexToLocFormat: short offsets
    i16be(&mut t, 0); // glyphDataFormat
    t
}

fn hhea() -> Vec<u8> {
    let mut t = Vec::with_capacity(36);
    u32be(&mut t, 0x0001_0000);
    i16be(&mut t, 800); // ascender
    i16be(&mut t, -200); // descender
    i16be(&mut t, 0); // lineGap
    u16be(&mut t, 500); // advanceWidthMax
    i16be(&mut t, 0); // minLeftSideBearing
    i16be(&mut t, 490); // minRightSideBearing
    i16be(&mut t, 10); // xMaxExtent
    i16be(&mut t, 1); // caretSlopeRise
    i16be(&mut t, 0); // caretSlopeRun
    i16be(&mut t, 0); // caretOffset
    t.extend_from_slice(&[0; 8]); // reserved
    i16be(&mut t, 0); // metricDataFormat
    u16be(&mut t, 1); // numberOfHMetrics
    t
}

fn maxp() -> Vec<u8> {
    let mut t = Vec::with_capacity(32);
    u32be(&mut t, 0x0001_0000);
    u16be(&mut t, 1); // numGlyphs
    u16be(&mut t, 3); // maxPoints
    u16be(&mut t, 1); // maxContours
    u16be(&mut t, 0); // maxCompositePoints
    u16be(&mut t, 0); // maxCompositeContours
    u16be(&mut t, 2); // maxZones
    for _ in 0..8 {
        u16be(&mut t, 0);
    }
    t
}

fn os2() -> Vec<u8> {
    let mut t = Vec::with_capacity(96);
    u16be(&mut t, 3); // version
    i16be(&mut t, 500); // xAvgCharWidth
    u16be(&mut t, 400); // usWeightClass
    u16be(&mut t, 5); // usWidthClass
    u16be(&mut t, 0); // fsType
    t.extend_from_slice(&[0; 20]); // subscript, superscript and strikeout metrics
    i16be(&mut t, 0); // sFamilyClass
    t.extend_from_slice(&[0; 10]); // panose
    t.extend_from_slice(&[0; 16]); // ulUnicodeRange1..4
    t.extend_from_slice(b"NONE"); // achVendID
    u16be(&mut t, 0x0040); // fsSelection: regular
    u16be(&mut t, 0xFFFF); // usFirstCharIndex
    u16be(&mut t, 0xFFFF); // usLastCharIndex
    i16be(&mut t, 800); // sTypoAscender
    i16be(&mut t, -200); // sTypoDescender
    i16be(&mut t, 0); // sTypoLineGap
    u16be(&mut t, 800); // usWinAscent
    u16be(&mut t, 200); // usWinDescent
    t.extend_from_slice(&[0; 8]); // ulCodePageRange1..2
    i16be(&mut t, 0); // sxHeight
    i16be(&mut t, 0); // sCapHeight
    u16be(&mut t, 0); // usDefaultChar
    u16be(&mut t, 0); // usBreakChar
    u16be(&mut t, 0); // usMaxContext
    t
}

/// A character map with the mandatory final segment only: no character has a glyph.
fn cmap() -> Vec<u8> {
    let mut t = Vec::with_capacity(36);
    u16be(&mut t, 0); // version
    u16be(&mut t, 1); // numTables
    u16be(&mut t, 3); // platformID: Windows
    u16be(&mut t, 1); // encodingID: Unicode BMP
    u32be(&mut t, 12); // offset of the subtable
    u16be(&mut t, 4); // format
    u16be(&mut t, 24); // length
    u16be(&mut t, 0); // language
    u16be(&mut t, 2); // segCountX2
    u16be(&mut t, 2); // searchRange
    u16be(&mut t, 0); // entrySelector
    u16be(&mut t, 0); // rangeShift
    u16be(&mut t, 0xFFFF); // endCode
    u16be(&mut t, 0); // reservedPad
    u16be(&mut t, 0xFFFF); // startCode
    u16be(&mut t, 1); // idDelta
    u16be(&mut t, 0); // idRangeOffset
    t
}

/// The single `.notdef` glyph: a small triangle.
fn glyf() -> Vec<u8> {
    let mut t = Vec::with_capacity(30);
    i16be(&mut t, 1); // numberOfContours
    for value in [0, 0, 10, 10] {
        i16be(&mut t, value); // bounding box
    }
    u16be(&mut t, 2); // endPtsOfContours
    u16be(&mut t, 0); // instructionLength
    t.extend_from_slice(&[1, 1, 1]); // flags: on-curve points with 16-bit deltas
    for value in [0, 10, -10] {
        i16be(&mut t, value); // x deltas
    }
    for value in [0, 0, 10] {
        i16be(&mut t, value); // y deltas
    }
    t.push(0); // pad to an even length, as short `loca` offsets require
    t
}

fn name() -> Vec<u8> {
    let records: [(u16, &str); 4] = [(1, "Blank"), (2, "Regular"), (4, "Blank"), (6, "Blank")];
    let mut strings = Vec::new();
    let mut t = Vec::new();
    u16be(&mut t, 0); // format
    u16be(&mut t, records.len() as u16);
    u16be(&mut t, 6 + 12 * records.len() as u16); // stringOffset
    for (id, text) in records {
        let encoded: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        u16be(&mut t, 3); // platformID
        u16be(&mut t, 1); // encodingID
        u16be(&mut t, 0x0409); // languageID: en-US
        u16be(&mut t, id);
        u16be(&mut t, encoded.len() as u16);
        u16be(&mut t, strings.len() as u16);
        strings.extend(encoded);
    }
    t.extend(strings);
    t
}

fn post() -> Vec<u8> {
    let mut t = Vec::with_capacity(32);
    u32be(&mut t, 0x0003_0000); // version 3: no glyph names
    t.extend_from_slice(&[0; 28]);
    t
}

/// Builds the blank font.
pub fn blank_font() -> Vec<u8> {
    let glyf = glyf();
    let mut loca = Vec::new();
    u16be(&mut loca, 0);
    u16be(&mut loca, (glyf.len() / 2) as u16);
    let mut hmtx = Vec::new();
    u16be(&mut hmtx, 500); // advanceWidth
    i16be(&mut hmtx, 0); // leftSideBearing

    // Tables in the alphabetical order of their tags, as the directory must list them.
    let tables: [(&[u8; 4], Vec<u8>); 10] = [
        (b"OS/2", os2()),
        (b"cmap", cmap()),
        (b"glyf", glyf),
        (b"head", head()),
        (b"hhea", hhea()),
        (b"hmtx", hmtx),
        (b"loca", loca),
        (b"maxp", maxp()),
        (b"name", name()),
        (b"post", post()),
    ];

    let count = tables.len() as u16;
    let entry_selector = 15 - count.leading_zeros() as u16;
    let search_range = (1u16 << entry_selector) * 16;
    let mut font = Vec::new();
    u32be(&mut font, 0x0001_0000);
    u16be(&mut font, count);
    u16be(&mut font, search_range);
    u16be(&mut font, entry_selector);
    u16be(&mut font, count * 16 - search_range);

    let mut offset = 12 + 16 * tables.len();
    let mut head_offset = 0;
    let mut body = Vec::new();
    for (tag, data) in &tables {
        if *tag == b"head" {
            head_offset = offset;
        }
        font.extend_from_slice(*tag);
        u32be(&mut font, checksum(data));
        u32be(&mut font, offset as u32);
        u32be(&mut font, data.len() as u32);
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    font.extend(body);

    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&font));
    font[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    font
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blank_font_is_well_formed() {
        let font = blank_font();
        assert_eq!(&font[..4], &[0, 1, 0, 0]);
        assert_eq!(font.len() % 4, 0);
        // The checksum of the whole font, adjustment included, is a fixed magic number.
        assert_eq!(checksum(&font), 0xB1B0_AFBA);
        // Every table lies inside the file.
        let count = u16::from_be_bytes([font[4], font[5]]) as usize;
        for index in 0..count {
            let record = &font[12 + 16 * index..28 + 16 * index];
            let offset = u32::from_be_bytes(record[8..12].try_into().unwrap()) as usize;
            let length = u32::from_be_bytes(record[12..16].try_into().unwrap()) as usize;
            assert!(offset + length <= font.len());
            assert_eq!(offset % 4, 0);
        }
    }
}
