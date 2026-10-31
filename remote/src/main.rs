//! loom's remote bridge: loom's agents on your other devices, over iroh (end-to-end encrypted;
//! relays carry only encrypted traffic). `loom-remote bridge` is started by loom; the rest is a
//! paired device on the command line, standing in for the phone app.

mod bridge;
mod loom;
mod peer;

const USAGE: &str = "loom-remote: your loom agents from another device.

  loom-remote pair <ticket> [name]     pair with loom (the ticket under Settings → Remote)
  loom-remote agents                   the agents, with their open questions
  loom-remote send <agent> <text…>     a message to an agent
  loom-remote answer <agent> <serial> <option>
  loom-remote watch                    what the agents do, as it happens; type
                                       <agent> <text> or answer <agent> <serial> <option>

loom itself starts `loom-remote bridge`.";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let number = |i: usize| args.get(i).and_then(|a| a.parse::<u64>().ok());
    let done = match args.first().map(String::as_str) {
        Some("bridge") => std::process::exit(bridge::run().await),
        Some("pair") if args.len() >= 2 => peer::pair(&args[1], args.get(2).map_or("loom-remote", String::as_str)).await,
        Some("agents") => peer::agents().await,
        Some("send") if args.len() >= 3 && number(1).is_some() => peer::send(number(1).unwrap_or(0), &args[2..].join(" ")).await,
        Some("answer") if number(1).zip(number(2)).zip(number(3)).is_some() => peer::answer(number(1).unwrap_or(0), number(2).unwrap_or(0), number(3).unwrap_or(0)).await,
        Some("watch") => peer::watch().await,
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(err) = done {
        eprintln!("loom-remote: {err}");
        std::process::exit(1);
    }
}
