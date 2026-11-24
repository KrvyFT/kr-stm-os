use std::{env, ffi::OsStr, path::PathBuf};

pub(super) const USAGE: &str = "Usage: pack -o <IMAGE> [--] [ELF ...]\n\n\
    Pack ARM ELF files into a KAPP v1 image in the given boot order.\n\n\
    Options:\n  -o, --output <IMAGE>  Image output path\n  -h, --help            Print help";

pub(super) struct Args {
    pub(super) output: PathBuf,
    pub(super) elfs: Vec<PathBuf>,
}

pub(super) fn parse_args() -> Result<Option<Args>, String> {
    let mut output = None;
    let mut elfs = Vec::new();
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_encoded_bytes() {
            b"--" => {
                elfs.extend(args.map(PathBuf::from));
                break;
            }
            b"-h" | b"--help" => return Ok(None),
            b"-o" | b"--output" => {
                let path = args
                    .next()
                    .filter(|arg| arg == "-" || !arg.as_encoded_bytes().starts_with(b"-"))
                    .ok_or("missing output path")?;
                output = Some(PathBuf::from(path));
            }
            bytes if bytes.starts_with(b"--output=") => {
                output = Some(attached_output(&bytes[b"--output=".len()..]));
            }
            bytes if bytes.starts_with(b"-o") => {
                let value = &bytes[b"-o".len()..];
                output = Some(attached_output(value.strip_prefix(b"=").unwrap_or(value)));
            }
            bytes if bytes.starts_with(b"-") && bytes != b"-" => {
                return Err(format!("unknown option {}", arg.to_string_lossy()));
            }
            _ => elfs.push(PathBuf::from(arg)),
        }
    }
    let output = output.ok_or("missing -o <IMAGE>")?;
    Ok(Some(Args { output, elfs }))
}

fn attached_output(encoded_path: &[u8]) -> PathBuf {
    // SAFETY: These bytes came from OsStr::as_encoded_bytes and were split
    // immediately after a known nonempty ASCII option prefix.
    PathBuf::from(unsafe { OsStr::from_encoded_bytes_unchecked(encoded_path) })
}
