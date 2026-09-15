// SPDX-License-Identifier: MPL-2.0
//! Worker-only bounded reads of immutable private owned storage.
use crate::cancellation::Cancellation;
use bareline_document::source::{MemorySource, SourceRead};
use std::{io, ops::Range};

pub(crate) fn read_exact(
    source: &MemorySource,
    mut offset: u64,
    mut output: &mut [u8],
    cancel: &Cancellation,
) -> io::Result<()> {
    if offset
        .checked_add(output.len() as u64)
        .is_none_or(|end| end > source.len())
    {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "owned source range"));
    }
    while !output.is_empty() {
        cancel
            .check()
            .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "owned read cancelled"))?;
        let page = source.page_size() as u64;
        let count = output.len().min((page - offset % page) as usize);
        match source.read(offset..offset + count as u64) {
            SourceRead::Ready(bytes) => {
                output[..count].copy_from_slice(bytes.bytes());
                offset += count as u64;
                output = &mut output[count..];
            }
            SourceRead::Pending(ticket) => {
                if !source
                    .resolve_owned(ticket)
                    .map_err(|error| io::Error::other(format!("{error:?}")))?
                {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "owned source loader unavailable",
                    ));
                }
            }
            SourceRead::Unavailable(reason) => {
                return Err(io::Error::other(format!("owned source unavailable: {reason:?}")));
            }
        }
    }
    Ok(())
}
/// UTF-8 boundaries may cross source pages. Only an 8 KiB stack buffer is retained.
pub(crate) fn visit_utf8<E: From<io::Error>>(
    source: &MemorySource,
    range: Range<u64>,
    cancel: &Cancellation,
    mut visit: impl FnMut(&str) -> Result<(), E>,
) -> Result<(), E> {
    if range.start > range.end || range.end > source.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "owned source range").into());
    }
    let mut buffer = [0u8; 8196];
    let mut carried = 0;
    let mut offset = range.start;
    while offset < range.end {
        let count = (range.end - offset).min(8192) as usize;
        read_exact(source, offset, &mut buffer[carried..carried + count], cancel)?;
        let length = carried + count;
        let valid = match std::str::from_utf8(&buffer[..length]) {
            Ok(_) => length,
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "owned source is not UTF-8").into()),
        };
        if valid > 0 {
            visit(
                std::str::from_utf8(&buffer[..valid])
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            )?;
        }
        buffer.copy_within(valid..length, 0);
        carried = length - valid;
        offset += count as u64;
    }
    if carried != 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "owned source ends inside UTF-8 scalar").into());
    }
    Ok(())
}
