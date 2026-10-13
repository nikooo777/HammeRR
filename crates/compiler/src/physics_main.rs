use std::process::ExitCode;

fn main() -> ExitCode {
    match compiler::physics::run_worker() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
