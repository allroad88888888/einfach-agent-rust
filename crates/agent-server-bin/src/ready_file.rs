//! Java 等父进程使用的启动就绪文件。
//!
//! `--ready-file <path>` 在成功 bind 后才发布一行 UTF-8 JSON：
//! `{"port":43123,"pid":123,"version":"0.1.0"}\n`。父进程必须为每次启动
//! 选择一个尚不存在的路径；本模块先在同目录创建独占临时文件，再原子创建一个
//! 指向它的硬链接作为目标路径，所以观察者只会看到完整文件或看不到文件，绝不
//! 需要解析启动日志。

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const TEMP_FILE_ATTEMPTS: u16 = 32;

/// 在 `path` 原子发布当前进程的就绪记录。
///
/// 原子硬链接要求目标文件尚不存在，这是刻意的生命周期契约：父进程应给每个子
/// 进程一个新的路径，陈旧文件不能被悄悄当作本次启动成功。
pub fn publish(path: &Path, port: u16) -> io::Result<()> {
    let json = format!(
        "{{\"port\":{port},\"pid\":{},\"version\":\"{VERSION}\"}}\n",
        std::process::id()
    );
    write_atomically(path, json.as_bytes())
}

fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    reject_existing_target(path)?;
    let (temporary_path, mut temporary_file) = create_temporary_file(path)?;
    if let Err(error) = temporary_file
        .write_all(contents)
        .and_then(|()| temporary_file.sync_all())
    {
        drop(temporary_file);
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    drop(temporary_file);
    // `rename` 在 Unix 上会覆盖已有目标；而 `hard_link` 以「目标必须不存在」
    // 的原子条件发布同一个 inode，既保留完整写入，也不让并发留下的陈旧文件
    // 被覆盖。临时文件和目标文件同目录，保证处于同一文件系统。
    let result = fs::hard_link(&temporary_path, path);
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
        return result;
    }
    fs::remove_file(temporary_path)
}

fn reject_existing_target(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "--ready-file {} 已存在；每次启动必须使用新路径",
                path.display()
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn create_temporary_file(path: &Path) -> io::Result<(PathBuf, File)> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--ready-file 必须是文件路径，不能是目录",
            )
        })?;
    let name = name.to_string_lossy();

    for attempt in 0..TEMP_FILE_ATTEMPTS {
        let temporary_path = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), attempt));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("无法为 --ready-file {} 创建独占临时文件", path.display()),
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{
            Arc, Barrier,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::*;

    static TEST_DIRECTORY_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    fn test_directory() -> PathBuf {
        let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agent-server-ready-file-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn publish_writes_the_documented_json_record() {
        let directory = test_directory();
        let path = directory.join("server.ready");

        publish(&path, 43123).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!(
                "{{\"port\":43123,\"pid\":{},\"version\":\"{VERSION}\"}}\n",
                std::process::id()
            )
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn atomic_write_leaves_no_temporary_file_behind() {
        let directory = test_directory();
        let path = directory.join("server.ready");

        write_atomically(&path, b"complete\n").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"complete\n");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stale_ready_file_is_rejected_instead_of_being_replaced() {
        let directory = test_directory();
        let path = directory.join("server.ready");
        fs::write(&path, b"stale\n").unwrap();

        let error = publish(&path, 43123).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"stale\n");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concurrent_publishers_cannot_replace_each_others_record() {
        let directory = test_directory();
        let path = directory.join("server.ready");
        let barrier = Arc::new(Barrier::new(2));
        let publishers = [43123, 43124].map(|port| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                publish(&path, port)
            })
        });

        let results = publishers.map(|publisher| publisher.join().unwrap());
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(results.iter().any(|result| {
            result
                .as_ref()
                .is_err_and(|error| error.kind() == io::ErrorKind::AlreadyExists)
        }));
        let record = fs::read_to_string(&path).unwrap();
        assert!(record.starts_with("{\"port\":43123,") || record.starts_with("{\"port\":43124,"));
        fs::remove_dir_all(directory).unwrap();
    }
}
