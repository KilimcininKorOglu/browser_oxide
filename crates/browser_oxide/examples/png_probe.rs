//! Sweep deflate parameters (level x strategy x memLevel x feed pattern)
//! against the real Chrome canvas PNG capture, via libz-rs-sys.
use libz_rs_sys::{deflate, deflateEnd, deflateInit2_, z_stream};
use std::ffi::CString;
use std::io::Read;

fn deflate_with(
    raw: &[u8],
    row_len: usize,
    level: i32,
    mem_level: i32,
    strategy: i32,
    per_row: bool,
) -> Option<Vec<u8>> {
    unsafe {
        let mut strm: z_stream = std::mem::zeroed();
        let version = CString::new("1.2.11").unwrap();
        let rc = deflateInit2_(
            &mut strm, level, 8, 15, mem_level, strategy, version.as_ptr(),
            std::mem::size_of::<z_stream>() as i32,
        );
        if rc != 0 {
            return None;
        }
        let mut out = vec![0u8; 65536];
        let mut stream = Vec::new();
        let mut feed = |input: &[u8], finish: bool| -> bool {
            let mut off = 0usize;
            loop {
                strm.next_in = input[off..].as_ptr() as *mut u8;
                strm.avail_in = (input.len() - off) as u32;
                strm.next_out = out.as_mut_ptr();
                strm.avail_out = out.len() as u32;
                let rc = deflate(
                    &mut strm,
                    if finish {
                        libz_rs_sys::Z_FINISH
                    } else {
                        libz_rs_sys::Z_NO_FLUSH
                    },
                );
                let produced = out.len() - strm.avail_out as usize;
                stream.extend_from_slice(&out[..produced]);
                off += (input.len() - off) - strm.avail_in as usize;
                if rc == libz_rs_sys::Z_STREAM_END {
                    return true;
                }
                if rc != libz_rs_sys::Z_OK && rc != libz_rs_sys::Z_BUF_ERROR {
                    return false;
                }
                if strm.avail_in == 0 && produced == 0 && !finish {
                    return true;
                }
                if finish && produced == 0 && rc == libz_rs_sys::Z_BUF_ERROR {
                    return false;
                }
            }
        };
        let ok = if per_row {
            for y in 0..raw.len() / row_len {
                if !feed(&raw[y * row_len..(y + 1) * row_len], false) {
                    return None;
                }
            }
            feed(&[], true)
        } else {
            feed(raw, true)
        };
        deflateEnd(&mut strm);
        if ok { Some(stream) } else { None }
    }
}

fn main() {
    let chrome = std::fs::read("/tmp/chrome_png.bin").unwrap();
    let mut idat = Vec::new();
    let mut i = 8;
    while i < chrome.len() {
        let ln = u32::from_be_bytes(chrome[i..i + 4].try_into().unwrap()) as usize;
        if &chrome[i + 4..i + 8] == b"IDAT" {
            idat.extend_from_slice(&chrome[i + 8..i + 8 + ln]);
        }
        i += 12 + ln;
    }
    let mut decomp = flate2::read::ZlibDecoder::new(&idat[..]);
    let mut raw = Vec::new();
    decomp.read_to_end(&mut raw).unwrap();
    let row_len = 1 + 60 * 4;

    println!("chrome idat: {} bytes", idat.len());
    let strategies = [("default", 0i32), ("filtered", 1), ("huffman", 2), ("rle", 3), ("fixed", 4)];
    for level in [1, 2, 3, 4, 5, 6] {
        for (sname, sid) in strategies {
            for mem in [8, 9, 7] {
                for per_row in [true, false] {
                    if let Some(stream) = deflate_with(&raw, row_len, level, mem, sid, per_row) {
                        if stream == idat {
                            println!(
                                "EXACT MATCH: level {} strategy {} memLevel {} per_row {}",
                                level, sname, mem, per_row
                            );
                            return;
                        }
                        if stream.len() == idat.len() {
                            println!(
                                "len match only: level {} strategy {} memLevel {} per_row {}",
                                level, sname, mem, per_row
                            );
                        }
                    }
                }
            }
        }
    }
    println!("no exact match in sweep");
}
