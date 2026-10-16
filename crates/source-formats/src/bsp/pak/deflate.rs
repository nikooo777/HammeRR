use std::io;

use flate2::{Decompress, FlushDecompress, Status};

use super::{CHUNK_BYTES, invalid};

pub(super) fn validate(
    data: &[u8],
    expected_size: usize,
    check: &dyn Fn() -> io::Result<()>,
) -> io::Result<()> {
    let mut decoder = Decompress::new(false);
    let mut buffer = [0; CHUNK_BYTES];
    loop {
        check()?;
        let before_in = decoder.total_in() as usize;
        let before_out = decoder.total_out();
        let end = data.len().min(before_in.saturating_add(CHUNK_BYTES));
        let status = decoder
            .decompress(&data[before_in..end], &mut buffer, FlushDecompress::None)
            .map_err(|error| invalid(format!("PAK DEFLATE stream is invalid: {error}")))?;
        if decoder.total_out() > expected_size as u64 {
            return Err(invalid("PAK DEFLATE exceeds its advertised decoded size"));
        }
        if status == Status::StreamEnd {
            if decoder.total_in() != data.len() as u64
                || decoder.total_out() != expected_size as u64
            {
                return Err(invalid(
                    "PAK DEFLATE stream has trailing bytes or an invalid length",
                ));
            }
            return Ok(());
        }
        if decoder.total_in() == before_in as u64 && decoder.total_out() == before_out {
            return Err(invalid(
                "PAK DEFLATE stream is truncated or makes no progress",
            ));
        }
    }
}
