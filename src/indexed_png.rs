use anyhow::{Context, Result, bail};
use png::{BitDepth, ColorType, Encoder};
use std::{fs::File, io::BufWriter, path::Path};

fn bit_depth_for_palette(colors: usize) -> Result<(BitDepth, u8)> {
    match colors {
        0 => bail!("an indexed PNG requires at least one palette color"),
        1..=2 => Ok((BitDepth::One, 1)),
        3..=4 => Ok((BitDepth::Two, 2)),
        5..=16 => Ok((BitDepth::Four, 4)),
        17..=256 => Ok((BitDepth::Eight, 8)),
        _ => bail!("an indexed PNG supports at most 256 palette colors"),
    }
}

fn pack_indices(indices: &[u8], width: u32, height: u32, bits: u8) -> Result<Vec<u8>> {
    let pixel_count = width as usize * height as usize;
    if indices.len() != pixel_count {
        bail!(
            "indexed pixel buffer has {} entries, expected {pixel_count}",
            indices.len()
        );
    }
    let row_bytes = (width as usize * bits as usize).div_ceil(8);
    let mut packed = vec![0_u8; row_bytes * height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let bit_offset = x * bits as usize;
            let byte = y * row_bytes + bit_offset / 8;
            let shift = 8 - bits as usize - bit_offset % 8;
            packed[byte] |= indices[y * width as usize + x] << shift;
        }
    }
    Ok(packed)
}

/// Save one palette index per input pixel as a lossless indexed-color PNG.
/// Returns the number of bits used for each stored palette index.
pub fn save(
    path: &Path,
    width: u32,
    height: u32,
    indices: &[u8],
    palette: &[[u8; 3]],
) -> Result<u8> {
    let (depth, bits) = bit_depth_for_palette(palette.len())?;
    if let Some(&index) = indices
        .iter()
        .find(|&&index| index as usize >= palette.len())
    {
        bail!(
            "pixel palette index {index} is outside a {}-color palette",
            palette.len()
        );
    }
    let packed = pack_indices(indices, width, height, bits)?;
    let palette_bytes: Vec<u8> = palette.iter().flatten().copied().collect();
    let file = File::create(path)
        .with_context(|| format!("could not create indexed PNG {}", path.display()))?;
    let mut encoder = Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(ColorType::Indexed);
    encoder.set_depth(depth);
    encoder.set_palette(palette_bytes);
    let mut writer = encoder
        .write_header()
        .context("could not write indexed PNG header")?;
    writer
        .write_image_data(&packed)
        .context("could not write indexed PNG pixels")?;
    Ok(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_the_smallest_legal_bit_depth() {
        assert_eq!(bit_depth_for_palette(2).unwrap().1, 1);
        assert_eq!(bit_depth_for_palette(3).unwrap().1, 2);
        assert_eq!(bit_depth_for_palette(16).unwrap().1, 4);
        assert_eq!(bit_depth_for_palette(17).unwrap().1, 8);
        assert!(bit_depth_for_palette(257).is_err());
    }

    #[test]
    fn packs_each_scanline_from_its_most_significant_bit() {
        let one_bit = pack_indices(&[0, 1, 1, 0, 1, 0, 0, 1], 4, 2, 1).unwrap();
        assert_eq!(one_bit, [0b0110_0000, 0b1001_0000]);

        let four_bit = pack_indices(&[1, 2, 3], 3, 1, 4).unwrap();
        assert_eq!(four_bit, [0x12, 0x30]);
    }
}
