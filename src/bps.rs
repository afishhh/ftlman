//! Implements the patching algorithm for BPS patch files (documented [here]).
//!
//! [here]: https://github.com/blakesmith/rombp/blob/master/docs/bps_spec.md
use std::hash::Hasher;

use anyhow::{Context, Result, bail};

fn take_one(patch: &mut &[u8]) -> Option<u8> {
    if let Some((first, tail)) = patch.split_first() {
        *patch = tail;
        Some(*first)
    } else {
        None
    }
}

fn take_varint(patch: &mut &[u8]) -> Result<usize> {
    let mut data = 0;
    let mut shift = 1;
    loop {
        let x = take_one(patch).context("Unexpected EOF while reading varint from patch")?;
        data += (x & 0x7f) as usize * shift;
        if (x & 0x80) != 0 {
            break;
        }
        shift <<= 7;
        data += shift;
    }

    Ok(data)
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

fn check_spare_capacity(target: &Vec<u8>, required: usize) -> Result<()> {
    if required > target.capacity() - target.len() {
        bail!("outputOffset exceeded preallocated target capacity");
    } else {
        Ok(())
    }
}

fn copy_into(target: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    check_spare_capacity(target, bytes.len())?;

    unsafe {
        let ptr = target.spare_capacity_mut().as_mut_ptr() as *mut u8;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        target.set_len(target.len() + bytes.len());
    }

    Ok(())
}

fn copy_into_within(target: &mut Vec<u8>, read_offset: usize, length: usize) -> Result<()> {
    check_spare_capacity(target, length)?;

    unsafe {
        let mut write_ptr = target.spare_capacity_mut().as_mut_ptr() as *mut u8;
        let mut read_ptr = target.as_mut_ptr().add(read_offset);
        // NOTE: std::ptr::copy doesn't work here
        //       Probably because it requires that after reading from read_ptr, write_ptr
        //       will not alias read_ptr.
        //       (at least that's how I understand "must not be subject to aliasing restrictions")
        for _ in 0..length {
            write_ptr.write(read_ptr.read());
            write_ptr = write_ptr.add(1);
            read_ptr = read_ptr.add(1);
        }
        target.set_len(target.len() + length);
    }

    Ok(())
}

const MAGIC: &[u8] = b"BPS1";

#[derive(Clone, Copy)]
pub struct Patch<'a> {
    pub metadata: &'a [u8],
    patch: &'a [u8],
    pub source_size: usize,
    pub source_crc: u32,
    pub target_size: usize,
    pub target_crc: u32,
    pub patch_crc: u32,
}

impl<'a> Patch<'a> {
    pub fn open(patch: &'a [u8]) -> Result<Self> {
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

        let source_size = take_varint(&mut patch)? as usize;
        let target_size = take_varint(&mut patch)? as usize;
        let metadata_size = take_varint(&mut patch)? as usize;

        let Some((metadata, actions)) = patch.split_at_checked(metadata_size) else {
            bail!("Metadata size exceeds remaining size of patch");
        };

        Ok(Patch {
            metadata,
            patch: actions,
            source_size,
            source_crc,
            target_size,
            target_crc,
            patch_crc,
        })
    }
}

impl Patch<'_> {
    pub fn patch(&self, target: &mut Vec<u8>, source: &[u8]) -> Result<()> {
        let Self {
            metadata: _,
            mut patch,
            source_size,
            source_crc,
            target_size,
            target_crc,
            patch_crc: _,
        } = *self;

        if crc32fast::hash(source) != source_crc {
            bail!("Source checksum doesn't match")
        }

        if source.len() != source_size {
            bail!("Source size doesn't match")
        }

        reserve_to_size(target, target_size);

        let mut source_relative_offset: usize = 0;
        let mut target_relative_offset: usize = 0;

        while !patch.is_empty() {
            let word = take_varint(&mut patch)?;
            let action = word & 0b11;
            let length = (word >> 2) + 1;
            match action {
                // SourceRead
                0 => {
                    let data = source
                        .get(target.len()..target.len() + length)
                        .context("SourceRead went out of bounds of source")?;

                    copy_into(target, data)?;
                }
                // TargetRead
                1 => {
                    let (data, remaining) = patch
                        .split_at_checked(length)
                        .context("TargetRead went out of bounds of patch")?;

                    copy_into(target, data)?;
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

                    copy_into(target, data)?;
                    source_relative_offset += length;
                }
                // TargetCopy
                3 => {
                    let offset = take_signed_varint(&mut patch)?;
                    target_relative_offset = target_relative_offset
                        .checked_add_signed(offset)
                        .context("TargetCopy underflowed targetRelativeOffset")?;
                    if target_relative_offset >= target.len() {
                        bail!("TargetCopy went out of bounds of target")
                    }

                    copy_into_within(target, target_relative_offset, length)?;
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
}

pub fn patch(target: &mut Vec<u8>, source: &[u8], patch: &[u8]) -> Result<()> {
    Patch::open(patch)?.patch(target, source)
}
