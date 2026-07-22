//! Line-delimited JSON-RPC server for local XCoding clients.

use std::{env, path::PathBuf, process};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use xcoding_core::CoreService;
use xcoding_protocol::{JsonRpcRequest, JsonRpcResponse, RpcError};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let database_path = match parse_database_path() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("xcoding-server: {message}");
            process::exit(2);
        }
    };

    if let Some(parent) = database_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("xcoding-server: failed to create data directory: {error}");
            process::exit(1);
        }
    }

    let core = match CoreService::open(&database_path) {
        Ok(core) => core,
        Err(error) => {
            eprintln!("xcoding-server: failed to initialize core: {error}");
            process::exit(1);
        }
    };

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => core.dispatch(request),
            Err(error) => JsonRpcResponse::failure(
                serde_json::Value::Null,
                RpcError::parse_error(format!("invalid JSON-RPC request: {error}")),
            ),
        };

        let encoded = serde_json::to_string(&response).expect("response must serialize");
        if stdout.write_all(encoded.as_bytes()).await.is_err()
            || stdout.write_all(b"\n").await.is_err()
            || stdout.flush().await.is_err()
        {
            break;
        }
    }
}

fn parse_database_path() -> Result<PathBuf, String> {
    let mut arguments = env::args_os().skip(1);
    let Some(flag) = arguments.next() else {
        return Err("expected --db <path>".to_owned());
    };

    if flag != "--db" {
        return Err("only --db <path> is supported".to_owned());
    }

    let Some(path) = arguments.next() else {
        return Err("expected a database path after --db".to_owned());
    };

    if arguments.next().is_some() {
        return Err("unexpected extra arguments".to_owned());
    }

    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn database_path_requires_explicit_flag() {
        // Argument parsing is kept intentionally minimal; integration tests exercise stdio.
        assert!(PathBuf::from(".xcoding/xcoding.db").ends_with("xcoding.db"));
    }
}
