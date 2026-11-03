//! `forme`, the machine's face: `forme validate <list.json> [--blobs <dir>]` holds a list to the validator, and `forme write <list.json> --blobs <dir> -o <out.pdf> [--records <out.json>] [--partial]` writes its file, and with `--records` the writer's records as Caster's `WriterRecords` JSON. Both read the committed fixture format: a list file without `resources.blobs`, beside a folder of blobs each named by its SHA-256 (`--blobs` defaults to the `blobs` folder beside the list). `--partial` leaves out the items this stage does not write and lists them on stderr, for the stage gates only. A refusal is printed on stderr and exits 1; a command line it cannot read exits 2.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: forme validate <list.json> [--blobs <dir>]\n       forme write <list.json> --blobs <dir> -o <out.pdf> [--records <out.json>] [--partial]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(message)) => {
            eprintln!("forme: {message}\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Refused(message)) => {
            eprintln!("{message}");
            ExitCode::from(1)
        }
    }
}

enum Failure {
    Usage(String),
    Refused(String),
}

/// The command line, read strictly: one verb, one list, the flags the verb takes, and nothing else.
struct Command {
    verb: String,
    list: PathBuf,
    blobs: Option<PathBuf>,
    out: Option<PathBuf>,
    records: Option<PathBuf>,
    partial: bool,
}

fn parse_args(args: &[String]) -> Result<Command, Failure> {
    let mut args = args.iter();
    let verb = args.next().ok_or_else(|| Failure::Usage("no command".into()))?.clone();
    if verb != "validate" && verb != "write" {
        return Err(Failure::Usage(format!("no command {verb:?}")));
    }
    let mut command = Command { verb, list: PathBuf::new(), blobs: None, out: None, records: None, partial: false };
    let mut list = None;
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().map(PathBuf::from).ok_or_else(|| Failure::Usage(format!("{flag} needs a value")));
        match arg.as_str() {
            "--blobs" => command.blobs = Some(value("--blobs")?),
            "-o" | "--out" if command.verb == "write" => command.out = Some(value(arg)?),
            "--records" if command.verb == "write" => command.records = Some(value("--records")?),
            "--partial" if command.verb == "write" => command.partial = true,
            flag if flag.starts_with('-') => return Err(Failure::Usage(format!("{} takes no flag {flag}", command.verb))),
            path if list.is_none() => list = Some(PathBuf::from(path)),
            extra => return Err(Failure::Usage(format!("one list only, and {extra:?} is a second"))),
        }
    }
    command.list = list.ok_or_else(|| Failure::Usage("no list".into()))?;
    if command.verb == "write" && (command.blobs.is_none() || command.out.is_none()) {
        return Err(Failure::Usage("write needs --blobs and -o".into()));
    }
    Ok(command)
}

fn run(args: &[String]) -> Result<(), Failure> {
    let command = parse_args(args)?;
    let read = |path: &Path| std::fs::read_to_string(path).map_err(|e| Failure::Refused(format!("forme: cannot read {}: {e}", path.display())));
    let list = forme::list::parse(&read(&command.list)?).map_err(|r| Failure::Refused(r.message))?;
    let dir = command.blobs.clone().unwrap_or_else(|| command.list.parent().unwrap_or(Path::new(".")).join("blobs"));
    let blobs = forme::read_blobs(&list, &dir).map_err(|e| Failure::Refused(format!("forme: cannot read the blobs in {}: {e}", dir.display())))?;
    if command.verb == "validate" {
        return forme::validate::validate(&list, &blobs).map_err(|r| Failure::Refused(r.message));
    }
    let written = if command.partial {
        let (written, left_out) = forme::write_partial(&list, &blobs).map_err(|r| Failure::Refused(r.message))?;
        for (kind, count) in left_out {
            if kind == "colour story" {
                eprintln!("forme: left out the colour story");
            } else {
                eprintln!("forme: left out {count} {kind} item{}", if count == 1 { "" } else { "s" });
            }
        }
        written
    } else {
        forme::write(&list, &blobs).map_err(|r| Failure::Refused(r.message))?
    };
    let save = |path: &Path, bytes: &[u8]| std::fs::write(path, bytes).map_err(|e| Failure::Refused(format!("forme: cannot write {}: {e}", path.display())));
    if let Some(records) = &command.records {
        save(records, serde_json::to_string(&written.records).expect("the records serialise").as_bytes())?;
    }
    save(&command.out.expect("checked by parse_args"), &written.bytes)
}
