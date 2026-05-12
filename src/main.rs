mod cli;

#[tokio::main]
async fn main() {
    if let Err(e) = cli::run().await {
        if let Some(user_err) = e.downcast_ref::<cli::CliError>() {
            eprintln!("Error: {user_err}");
        } else {
            eprintln!("{:?}", e);
        }
        std::process::exit(1);
    }
}
