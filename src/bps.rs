//! Implements the patching algorithm for BPS patch files (documented [here]).
//!
//! [here]: https://github.com/blakesmith/rombp/blob/master/docs/bps_spec.md
use std::hash::Hasher;

use anyhow::{bail, Context, Result};

trait PatchExt {
    fn take_one(&mut self) -> Option<u8>;
}

impl PatchExt for &[u8] {
    fn take_one(&mut self) -> Option<u8> {
        if let Some((first, tail)) = self.split_first() {
            *self = tail;
            Some(*first)
        } else {
            None
        }
    }
}

fn take_varint(patch: &mut &[u8]) -> Result<usize> {
    let mut data = 0;
    let mut shift = 1;
    loop {
        let x = patch
            .take_one()
            .context("Unexpected EOF while reading varint from patch")?;
        data += (x & 0x7f) as usize * shift;
        if (x & 0x80) != 0 {
            break;
        }
        shift <<= 7;
        data += shift;
    }

    return Ok(data);
}

fn take_signed_varint(patch: &mut &[u8]) -> Result<isize> {
    let data = take_varint(patch)?;
    let sign = -2 * (data & 1) as isize + 1;
    Ok(sign * (data >> 1) as isize)
}

fn take_crc_back(patch: &mut &[u8]) -> Result<u32> {
    let Some((new_patch, crc_bytes)) = patch.split_last_chunk::<4>() else {
        bail!("Unexpected EOF while reading CRC")
    };
    *patch = new_patch;
    Ok(u32::from_le_bytes(*crc_bytes))
}

fn reserve_to_size(target: &mut Vec<u8>, length: usize) {
    if let Some(additional) = length.checked_sub(target.len()) {
        target.reserve(additional);
    }
}

fn copy_into(target: &mut Vec<u8>, bytes: &[u8], offset: usize) {
    let new_size = offset + bytes.len();
    reserve_to_size(target, new_size);

    unsafe {
        let ptr = target.as_mut_ptr().add(offset);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        if new_size > target.len() {
            target.set_len(new_size);
        }
    }
}

fn copy_into_within(target: &mut Vec<u8>, write_offset: usize, read_offset: usize, length: usize) {
    let new_size = write_offset + length;
    reserve_to_size(target, new_size);

    unsafe {
        let mut write_ptr = target.as_mut_ptr().add(write_offset);
        let mut read_ptr = target.as_mut_ptr().add(read_offset);
        // NOTE: std::ptr::copy doesn't work here
        //       Probably because it requires that after reading from read_ptr, write_ptr
        //       will not alias read_ptr.
        //       (at least that's how I understand "must not be subject to aliasing restrictions")
        for _ in 0..length {
            *write_ptr = *read_ptr;
            write_ptr = write_ptr.add(1);
            read_ptr = read_ptr.add(1);
        }
        if new_size > target.len() {
            target.set_len(new_size);
        }
    }
}

const MAGIC: &[u8] = b"BPS1";

pub fn patch(target: &mut Vec<u8>, source: &[u8], patch: &[u8]) -> Result<()> {
    let Some(mut patch) = patch.strip_prefix(MAGIC) else {
        bail!("`patch` is not a valid BPS patch (mismatched magic)")
    };

    let patch_crc = take_crc_back(&mut patch)?;

    let crc = {
        let mut hasher = crc32fast::Hasher::new();
        hasher.write(MAGIC);
        hasher.write(patch);
        hasher.finalize()
    };

    if crc != patch_crc {
        bail!("Patch doesn't match checksum")
    }

    let target_crc = take_crc_back(&mut patch)?;
    let source_crc = take_crc_back(&mut patch)?;

    if crc32fast::hash(source) != source_crc {
        bail!("Source checksum doesn't match")
    }

    let source_size = take_varint(&mut patch)? as usize;
    let target_size = take_varint(&mut patch)? as usize;
    let metadata_size = take_varint(&mut patch)? as usize;

    let Some((_, mut patch)) = patch.split_at_checked(metadata_size) else {
        bail!("Metadata size exceeds remaining size of patch");
    };

    if source.len() != source_size {
        bail!("Source size doesn't match")
    }

    let mut output_offset = 0;
    let mut source_relative_offset: usize = 0;
    let mut target_relative_offset: usize = 0;

    while !patch.is_empty() {
        let word = take_varint(&mut patch)?;
        let action = word & 0b11;
        let length = (word >> 2) + 1;
        match action {
            // SourceRead
            0 => {
                if let Some(additional) = (output_offset + length).checked_sub(target.len()) {
                    target.reserve(additional);
                }
                let data = source
                    .get(output_offset..output_offset + length)
                    .context("SourceRead went out of bounds of source")?;

                copy_into(target, data, output_offset);
                output_offset += length;
            }
            // TargetRead
            1 => {
                let (data, remaining) = patch
                    .split_at_checked(length)
                    .context("TargetRead length larger than patch")?;

                copy_into(target, data, output_offset);
                output_offset += length;
                patch = remaining;
            }
            // SourceCopy
            2 => {
                let offset = take_signed_varint(&mut patch)?;
                source_relative_offset = source_relative_offset
                    .checked_add_signed(offset)
                    .context("SourceCopy underflowed sourceRelativeOffset")?;
                let data = source
                    .get(source_relative_offset..source_relative_offset + length)
                    .context("SourceCopy went out of bounds of source")?;

                copy_into(target, data, output_offset);
                output_offset += length;
                source_relative_offset += length;
            }
            // TargetCopy
            3 => {
                let offset = take_signed_varint(&mut patch)?;
                target_relative_offset = target_relative_offset
                    .checked_add_signed(offset)
                    .context("TargetCopy underflowed targetRelativeOffset")?;
                if target_relative_offset >= target.len() {
                    bail!("TargetCopy went out-of-bounds of target")
                }

                copy_into_within(target, output_offset, target_relative_offset, length);
                output_offset += length;
                target_relative_offset += length;
            }
            _ => unreachable!(),
        }
    }

    if target.len() != target_size {
        bail!("Target size doesn't match")
    }

    if crc32fast::hash(target) != target_crc {
        bail!("Target checksum doesn't match")
    }

    Ok(())
}
