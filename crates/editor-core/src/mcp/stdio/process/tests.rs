use super::*;
use std::io::{Read, Write};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::pipe;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn aliased_fifo_flags_restore_on_finish_drop_and_partial_construction_failure() {
    use rustix::fs::{CWD, Mode, fcntl_getfl, mkfifoat};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("duplex.fifo");
    mkfifoat(CWD, &path, Mode::RUSR | Mode::WUSR).unwrap();
    let fifo = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let original = fcntl_getfl(&fifo).unwrap();
    for finish in [true, false] {
        let (read, write, guard) = from_owned_fds(
            fifo.try_clone().unwrap().into(),
            fifo.try_clone().unwrap().into(),
        )
        .unwrap();
        assert!(
            fcntl_getfl(&fifo)
                .unwrap()
                .contains(rustix::fs::OFlags::NONBLOCK)
        );
        drop(read);
        drop(write);
        if finish {
            guard.finish().unwrap();
        } else {
            drop(guard);
        }
        assert_eq!(fcntl_getfl(&fifo).unwrap(), original);
    }
    let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    assert!(from_owned_fds(fifo.try_clone().unwrap().into(), socket.into()).is_err());
    assert_eq!(fcntl_getfl(&fifo).unwrap(), original);
}

#[test]
fn closing_a_full_os_output_pipe_drops_pending_codec_bytes_and_exits_the_runtime() {
    use rmcp::model::{CallToolResult, ContentBlock, JsonRpcMessage, RequestId, ServerResult};
    use rmcp::transport::Transport;
    let flags;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (input, output, process_flags) = runtime.block_on(async {
        let (input, receiver) = pipe::pipe().unwrap();
        let (sender, output) = pipe::pipe().unwrap();
        let input = input.into_blocking_fd().unwrap();
        let output = output.into_blocking_fd().unwrap();
        let read = receiver.into_blocking_fd().unwrap();
        let write = sender.into_blocking_fd().unwrap();
        let (read, write, flags) = from_owned_fds(read, write).unwrap();
        let (mut transport, observer) = super::super::transport(read, write);
        let id = RequestId::Number(901);
        let receipt = observer.arm(id.clone()).unwrap();
        let result = CallToolResult::success(vec![ContentBlock::text("a".repeat(8 * 1024 * 1024))]);
        let mut send = tokio::spawn(transport.send(JsonRpcMessage::response(
            ServerResult::CallToolResult(result),
            id,
        )));
        tokio::time::timeout(TIMEOUT, async {
            while rustix::io::ioctl_fionread(&output).unwrap() == 0 {
                assert!(
                    !send.is_finished(),
                    "transport ended before writing pipe data"
                );
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("transport did not write pipe data before the deadline");
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut send)
                .await
                .is_err()
        );
        assert!(rustix::io::ioctl_fionread(&output).unwrap() > 0);
        tokio::time::timeout(TIMEOUT, transport.close())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            tokio::time::timeout(TIMEOUT, send)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotConnected
        );
        assert_eq!(
            tokio::time::timeout(TIMEOUT, receipt.wait()).await.unwrap(),
            Err(super::super::StdioFenceError::TransportClosed)
        );
        drop(transport);
        (input, output, flags)
    });
    flags = process_flags;
    drop(runtime);
    flags.finish().unwrap();
    drop(input);
    drop(output);
}

#[tokio::test]
async fn owned_os_pipes_cancel_pending_reads_preserve_eof_and_restore_flags() {
    let (input, receiver) = pipe::pipe().unwrap();
    let (sender, output) = pipe::pipe().unwrap();
    let input = input.into_blocking_fd().unwrap();
    let output = output.into_blocking_fd().unwrap();
    let read = receiver.into_blocking_fd().unwrap();
    let write = sender.into_blocking_fd().unwrap();
    let probe = read.try_clone().unwrap();
    let previous = rustix::fs::fcntl_getfl(&probe).unwrap();
    let (mut read, mut write, guard) = from_owned_fds(read, write).unwrap();
    assert!(
        rustix::fs::fcntl_getfl(&probe)
            .unwrap()
            .contains(rustix::fs::OFlags::NONBLOCK)
    );
    let mut byte = [0];
    assert!(
        tokio::time::timeout(Duration::from_millis(10), read.read(&mut byte))
            .await
            .is_err()
    );
    let mut input = std::fs::File::from(input);
    input.write_all(b"a").unwrap();
    assert_eq!(
        tokio::time::timeout(TIMEOUT, read.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        1
    );
    assert_eq!(byte, *b"a");
    drop(input);
    assert_eq!(
        tokio::time::timeout(TIMEOUT, read.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    tokio::time::timeout(TIMEOUT, write.write_all(b"b"))
        .await
        .unwrap()
        .unwrap();
    let mut output = std::fs::File::from(output);
    output.read_exact(&mut byte).unwrap();
    assert_eq!(byte, *b"b");
    drop(read);
    drop(write);
    guard.finish().unwrap();
    assert_eq!(rustix::fs::fcntl_getfl(&probe).unwrap(), previous);
}

#[tokio::test]
async fn regular_files_keep_finite_eof_and_output_and_sockets_reject_explicitly() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("input");
    let target = directory.path().join("output");
    std::fs::write(&source, b"input").unwrap();
    let input = std::fs::File::open(source).unwrap();
    let output = std::fs::File::create(&target).unwrap();
    let (mut read, mut write, guard) = from_owned_fds(input.into(), output.into()).unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(TIMEOUT, read.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"input");
    tokio::time::timeout(TIMEOUT, write.write_all(b"output"))
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(TIMEOUT, write.flush())
        .await
        .unwrap()
        .unwrap();
    drop(read);
    drop(write);
    guard.finish().unwrap();
    assert_eq!(std::fs::read(target).unwrap(), b"output");
    let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let output = tempfile::tempfile().unwrap();
    let error = from_owned_fds(socket.into(), output.into()).err().unwrap();
    assert!(error.contains("stdin must be a readable pipe or regular file"));
}
