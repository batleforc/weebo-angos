//! `weebo-angos-importer`: prints weebo-si's Angos release as weebo-angos
//! CRs, to review then `kubectl apply -f -` (spec 6.10). Nothing is
//! written to the cluster.

use std::io::Read;
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use importer::{Options, import, to_yaml};

#[derive(Parser, Debug)]
#[command(
    name = "weebo-angos-importer",
    version,
    about = "Print weebo-si's Angos release as weebo-angos CRs"
)]
struct Args {
    /// The release's values: `helm get values --all -o yaml <release>`, or
    /// the chart's values.yaml (`-` for stdin).
    #[arg(long)]
    values: PathBuf,
    /// The namespace of every object: the release's.
    #[arg(long, short, default_value = "angos")]
    namespace: String,
    /// The instance's name (not `angos`: the chart's Service keeps it until
    /// the cutover).
    #[arg(long, default_value = "registry")]
    name: String,
    /// The Secret the instance reads its credentials from.
    #[arg(long, default_value = "angos-operator")]
    credentials: String,
    /// The SecretStore of the ExternalSecret that writes it.
    #[arg(long, default_value = "cred-store")]
    secret_store: String,
    /// Print no ExternalSecret: the credentials Secret is made otherwise.
    #[arg(long)]
    no_external_secret: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let text = if args.values.as_os_str() == "-" {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        text
    } else {
        std::fs::read_to_string(&args.values)
            .with_context(|| format!("reading {}", args.values.display()))?
    };
    let values: serde_json::Value =
        serde_norway::from_str(&text).context("the values aren't YAML")?;
    let options = Options {
        namespace: args.namespace,
        name: args.name,
        credentials: args.credentials,
        secret_store: (!args.no_external_secret).then_some(args.secret_store),
    };
    let imported = import(&values, &options)?;
    for warning in &imported.warnings {
        eprintln!("warning: {warning}");
    }
    print!("{}", to_yaml(&imported)?);
    Ok(())
}
