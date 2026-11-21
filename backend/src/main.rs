use std::io;
fn main() {
    if std::env::args().nth(1).as_deref()==Some("--data-root") {
        match modlock_helper::paths::data_root(&json::object!{}) {
            Ok(root)=>println!("{}",json::object!{root:root.to_string_lossy().as_ref()}.dump()),
            Err(e)=>{eprintln!("{}",e.message);std::process::exit(if e.kind=="settingsPermissionDenied" {3} else {1});}
        }
        return;
    }
    let stdin = io::stdin();
    let stdout = io::stdout();
    if let Err(error) = modlock_helper::protocol::serve(stdin.lock(), stdout.lock()) {
        if error.kind() != io::ErrorKind::BrokenPipe {
            eprintln!("modlock-helper: protocol I/O failed");
            std::process::exit(1);
        }
    }
}
