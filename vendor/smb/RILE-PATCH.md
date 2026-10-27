# Local SMB pipe API patch

Upstream: `smb` 0.10.2, MIT, commit
`ee2d4730468cf6ff7e7537ac7a441eab62946ced` of
https://github.com/AvivNaaman/smb-rs.

Only `src/resource/pipe.rs` differs from that release: `Pipe::write_raw` and
`Pipe::read_raw` expose the existing authenticated SMB pipe I/O. This lets Rile
implement NDR32 share enumeration for Samba/NAS servers that reject the SDK's
NDR64-only binding, without using its overly restrictive IOCTL response parser.
Signing, encryption, authentication and SMB connection sequencing are unchanged.

Remove this patch when the upstream crate exposes equivalent named-pipe I/O
or provides interoperable NDR32 share enumeration.
