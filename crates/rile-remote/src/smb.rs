//! SMB2/3 backend. `/` is a virtual server root containing disk shares;
//! `/share/path` addresses a file within a share. No system mounts or tools.

use std::io::Read;

mod srvsvc;

use anyhow::{Context, Result, bail};
use futures_util::TryStreamExt;
use smb::{
    Client, ClientConfig, CreateOptions, FileAccessMask, FileAttributes, FileCreateArgs,
    FileDirectoryInformation, FileDispositionInformation, FileNetworkOpenInformation,
    FileRenameInformation, ReadAt, Resource, ResourceHandle, UncPath, WriteAt,
};

use crate::{
    Auth, ConnectionInfo, RemoteEntry,
    backend::{Backend, CHUNK},
    paths,
};

pub(crate) struct SmbBackend {
    client: Option<Client>,
    connection: Option<std::sync::Arc<smb::Connection>>,
    runtime: tokio::runtime::Runtime,
    server: String,
    username: String,
    password: String,
}

pub(crate) fn is_connection_lost(error: &anyhow::Error) -> bool {
    error.chain().any(|error| {
        matches!(
            error.downcast_ref::<smb::Error>(),
            Some(
                smb::Error::ConnectionStopped
                    | smb::Error::OperationTimeout(..)
                    | smb::Error::IoError(_)
                    | smb::Error::TransportError(
                        smb::transport::TransportError::NotConnected
                            | smb::transport::TransportError::Timeout(_)
                            | smb::transport::TransportError::IoError(_)
                    )
            )
        )
    })
}

fn handle(resource: &Resource) -> &ResourceHandle {
    match resource {
        Resource::File(file) => file.handle(),
        Resource::Directory(dir) => dir.handle(),
        Resource::Pipe(pipe) => pipe.handle(),
    }
}

/// Reject SMB separators and alternate data streams supplied through remote paths.
fn split_path(path: &str) -> Result<(String, String)> {
    if path.contains(['\\', ':', '\0']) {
        bail!("Invalid SMB path");
    }
    let path = paths::normalize(path);
    let (share, rest) = path
        .trim_start_matches('/')
        .split_once('/')
        .unwrap_or((path.trim_start_matches('/'), ""));
    if share.is_empty() {
        bail!("Choose a share first");
    }
    Ok((share.to_string(), rest.replace('/', "\\")))
}

fn directory(path: &str) -> RemoteEntry {
    RemoteEntry {
        name: paths::file_name(path).to_string(),
        path: path.to_string(),
        is_dir: true,
        is_symlink: false,
        size: 0,
        modified: None,
        permissions: None,
    }
}

impl SmbBackend {
    pub(crate) fn connect(info: &ConnectionInfo, secret: Option<&str>) -> Result<Self> {
        if !matches!(info.auth, Auth::Password | Auth::Anonymous) {
            bail!("SMB supports password or guest authentication");
        }
        let mut config = ClientConfig::default();
        config.connection.port = Some(info.port);
        config.connection.timeout = Some(crate::CONNECT_TIMEOUT);
        config.connection.smb2_only_negotiate = true;
        config.connection.allow_unsigned_guest_access = info.auth == Auth::Anonymous;
        let client = Client::new(config);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let username = if info.auth == Auth::Anonymous {
            "guest"
        } else {
            &info.username
        }
        .to_string();
        let password = if info.auth == Auth::Anonymous {
            ""
        } else {
            secret.unwrap_or("")
        }
        .to_string();
        let mut backend = Self {
            client: Some(client),
            connection: None,
            runtime,
            server: info.host.clone(),
            username,
            password,
        };
        backend.connection = Some(
            backend
                .run(backend.client().connect(&info.host))
                .context("Connecting to SMB server")?,
        );
        // A specified share does not require permission to enumerate IPC$.
        if let Some(path) = &info.initial_path
            && paths::normalize(path) != "/"
        {
            backend.target(path)?;
        } else {
            backend
                .run(backend.client().ipc_connect(
                    &backend.server,
                    &backend.username,
                    backend.password.clone(),
                ))
                .context("Signing in to SMB server")?;
        }
        Ok(backend)
    }

    fn client(&self) -> &Client {
        self.client.as_ref().expect("live SMB client")
    }

    fn run<T>(&self, future: impl std::future::Future<Output = smb::Result<T>>) -> Result<T> {
        self.runtime.block_on(async {
            tokio::time::timeout(crate::IO_TIMEOUT, future)
                .await
                .map_err(|_| crate::ConnectionLost("SMB operation timed out".into()))?
                .map_err(Into::into)
        })
    }

    fn target(&self, path: &str) -> Result<UncPath> {
        let (share, rest) = split_path(path)?;
        let target = UncPath::new(&self.server)?.with_share(&share)?;
        self.run(
            self.client()
                .share_connect(&target, &self.username, self.password.clone()),
        )?;
        Ok(target.with_path(&rest))
    }

    /// Always close server handles, including after a failed or cancelled operation.
    fn with_resource<T>(
        &self,
        path: &str,
        args: FileCreateArgs,
        f: impl FnOnce(&Resource) -> Result<T>,
    ) -> Result<T> {
        let target = self.target(path)?;
        let resource = self.run(self.client().create_file(&target, &args))?;
        let result = f(&resource);
        let close = self.run(handle(&resource).close());
        let _guard = self.runtime.enter();
        drop(resource);
        let value = result?;
        close?;
        Ok(value)
    }

    fn delete(&self, path: &str) -> Result<()> {
        let (_, rest) = split_path(path)?;
        if rest.is_empty() {
            bail!("Cannot delete a share");
        }
        self.with_resource(
            path,
            FileCreateArgs::make_open_existing(FileAccessMask::new().with_delete(true)),
            |r| {
                self.run(handle(r).set_info(FileDispositionInformation::default()))?;
                Ok(())
            },
        )
    }
}

impl Backend for SmbBackend {
    fn login_dir(&mut self) -> Result<String> {
        Ok("/".into())
    }

    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        if paths::normalize(path) == "/" {
            self.run(self.client().ipc_connect(
                &self.server,
                &self.username,
                self.password.clone(),
            ))?;
            let shares = self
                .runtime
                .block_on(async {
                    tokio::time::timeout(
                        crate::IO_TIMEOUT,
                        srvsvc::list_shares(self.client(), &self.server),
                    )
                    .await
                    .map_err(|_| crate::ConnectionLost("SMB share listing timed out".into()))?
                })
                .context("Could not list SMB shares")?;
            return Ok(shares
                .into_iter()
                .map(|name| directory(&paths::join("/", &name)))
                .collect());
        }
        let target = self.target(path)?;
        let args =
            FileCreateArgs::make_open_existing(FileAccessMask::new().with_generic_read(true));
        let resource = self.run(self.client().create_file(&target, &args))?;
        let _guard = self.runtime.enter();
        let dir: smb::Directory = resource.try_into().map_err(|(error, _)| error)?;
        let dir = std::sync::Arc::new(dir);
        let mut entries = Vec::new();
        let listing = self.run(async {
            smb::Directory::query::<FileDirectoryInformation>(&dir, "*")
                .await?
                .try_collect::<Vec<_>>()
                .await
        });
        let close = self.run(dir.close());
        let listing = listing?;
        close?;
        for entry in listing {
            let name = entry.file_name.to_string();
            if name == "." || name == ".." {
                continue;
            }
            entries.push(RemoteEntry {
                path: paths::join(path, &name),
                name,
                is_dir: entry.file_attributes.directory(),
                is_symlink: entry.file_attributes.reparse_point(),
                size: entry.end_of_file,
                modified: Some(entry.last_write_time.into()),
                permissions: None,
            });
        }
        Ok(entries)
    }

    fn stat(&mut self, path: &str) -> Result<RemoteEntry> {
        if paths::normalize(path) == "/" {
            return Ok(directory("/"));
        }
        self.with_resource(
            path,
            FileCreateArgs::make_open_existing(
                FileAccessMask::new().with_file_read_attributes(true),
            ),
            |r| {
                let info = self.run(handle(r).query_info::<FileNetworkOpenInformation>())?;
                Ok(RemoteEntry {
                    is_dir: info.file_attributes.directory(),
                    is_symlink: info.file_attributes.reparse_point(),
                    size: info.end_of_file,
                    modified: Some(info.last_write_time.into()),
                    ..directory(path)
                })
            },
        )
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        self.with_resource(
            path,
            FileCreateArgs::make_create_new(
                FileAttributes::new().with_directory(true),
                CreateOptions::new().with_directory_file(true),
            ),
            |_| Ok(()),
        )
    }

    fn remove_file(&mut self, path: &str) -> Result<()> {
        self.delete(path)
    }
    fn remove_dir(&mut self, path: &str) -> Result<()> {
        self.delete(path)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let (from_share, from_path) = split_path(from)?;
        let (to_share, to_path) = split_path(to)?;
        if !from_share.eq_ignore_ascii_case(&to_share) {
            bail!("Cannot move between SMB shares; copy the file instead");
        }
        if from_path.is_empty() || to_path.is_empty() {
            bail!("Cannot rename a share");
        }
        self.with_resource(
            from,
            FileCreateArgs::make_open_existing(FileAccessMask::new().with_delete(true)),
            |r| {
                self.run(handle(r).set_info(FileRenameInformation {
                    replace_if_exists: false.into(),
                    root_directory: 0,
                    file_name: to_path.into(),
                }))?;
                Ok(())
            },
        )
    }

    fn read_file(&mut self, path: &str, sink: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<()> {
        self.with_resource(
            path,
            FileCreateArgs::make_open_existing(FileAccessMask::new().with_generic_read(true)),
            |r| {
                let file = r.as_file().context("Not a file")?;
                let size = self
                    .run(file.query_info::<FileNetworkOpenInformation>())?
                    .end_of_file;
                let mut offset = 0;
                let mut buffer = vec![0; CHUNK];
                while offset < size {
                    let len = buffer.len().min((size - offset) as usize);
                    let n = self.run(file.read_at(&mut buffer[..len], offset))?;
                    if n == 0 {
                        bail!("Unexpected end of SMB file");
                    }
                    offset += n as u64;
                    if !sink(&buffer[..n])? {
                        break;
                    }
                }
                Ok(())
            },
        )
    }

    fn write_file(&mut self, path: &str, src: &mut dyn Read) -> Result<()> {
        self.with_resource(
            path,
            FileCreateArgs::make_overwrite(
                FileAttributes::new(),
                CreateOptions::new().with_non_directory_file(true),
            ),
            |r| {
                let file = r.as_file().context("Not a file")?;
                let mut offset = 0;
                let mut buffer = vec![0; CHUNK];
                loop {
                    let n = src.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    let mut written = 0;
                    while written < n {
                        let count = self.run(file.write_at(&buffer[written..n], offset))?;
                        if count == 0 {
                            bail!("SMB write returned zero bytes");
                        }
                        written += count;
                        offset += count as u64;
                    }
                }
                Ok(())
            },
        )
    }

    fn is_alive(&self) -> bool {
        true
    }
}

impl Drop for SmbBackend {
    fn drop(&mut self) {
        // Closing the transport also releases server handles and sessions.
        // A graceful tree disconnect can wait forever for exhausted credits
        // after a connection failure, so do not send protocol requests here.
        if let Some(connection) = self.connection.take() {
            let _ = self.run(connection.close());
        }
        // SDK destructors schedule best-effort cleanup. Keep them inside the
        // runtime; dropping that runtime then cancels any outstanding requests.
        let _guard = self.runtime.enter();
        drop(self.client.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_paths() {
        assert_eq!(
            split_path("/Documents/Folder/file.txt").unwrap(),
            ("Documents".into(), "Folder\\file.txt".into())
        );
        assert_eq!(
            split_path("/Documents").unwrap(),
            ("Documents".into(), "".into())
        );
        assert!(split_path("/").is_err());
        assert!(split_path("/share/file:stream").is_err());
        assert!(split_path("/share/a\\b").is_err());
        assert!(split_path("/share/a\0b").is_err());
    }

    fn info(host: &str) -> ConnectionInfo {
        ConnectionInfo {
            id: "test".into(),
            name: "".into(),
            protocol: crate::Protocol::Smb,
            host: host.into(),
            port: 445,
            username: "alice".into(),
            auth: Auth::Password,
            initial_path: None,
        }
    }

    #[test]
    fn smb_addresses() {
        let mut address = info("smb://nas.local:1445/My%20Share/F%C3%B6lder");
        address.normalize_smb_address().unwrap();
        assert_eq!(address.host, "nas.local");
        assert_eq!(address.port, 1445);
        assert_eq!(address.initial_path.as_deref(), Some("/My Share/Földer"));
        let mut unc = info(r"\\nas\My Share\Folder");
        unc.normalize_smb_address().unwrap();
        assert_eq!(unc.host, "nas");
        assert_eq!(unc.initial_path.as_deref(), Some("/My Share/Folder"));
        let mut ipv6 = info("smb://[::1]/Share");
        ipv6.normalize_smb_address().unwrap();
        assert_eq!(ipv6.host, "::1");
        for invalid in [
            "ftp://nas/share",
            "smb://user:secret@nas/share",
            "smb://nas/share?query",
            "smb://nas/share#fragment",
        ] {
            assert!(info(invalid).normalize_smb_address().is_err());
        }
        let mut conflicting = info("smb://nas/one");
        conflicting.initial_path = Some("/two".into());
        assert!(conflicting.normalize_smb_address().is_err());
        assert_eq!(crate::Protocol::Smb.default_port(), 445);
        assert_eq!(crate::Protocol::Smb.label(), "SMB");
    }

    #[test]
    fn only_connection_failures_are_retried() {
        assert!(is_connection_lost(&smb::Error::ConnectionStopped.into()));
        assert!(is_connection_lost(
            &smb::Error::TransportError(smb::transport::TransportError::NotConnected).into()
        ));
        assert!(!is_connection_lost(
            &smb::Error::InvalidArgument("bad path".into()).into()
        ));
        assert!(!is_connection_lost(
            &std::io::Error::from(std::io::ErrorKind::PermissionDenied).into()
        ));
    }
}
