use editor_core::{
    mcp::{self, WorkspaceOwner},
    startup::{Application, Args},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::parse()?;
    args.headless = true;
    let application = Application::open(&args)?;
    if let Some(Err(message)) = &application.open_message {
        eprintln!("{message}");
    }
    let owner = match WorkspaceOwner::start(application.workspace(), None, None) {
        Ok(owner) => owner,
        Err(error) => return application.finish(Err(error.into())),
    };
    let result =
        mcp::serve_workspace_stdio(owner.server()).map_err(Box::<dyn std::error::Error>::from);
    application.finish(owner.finish(result))
}
