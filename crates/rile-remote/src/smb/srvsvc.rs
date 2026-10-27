//! NDR32 NetrShareEnum over the authenticated SMB IPC$ pipe (MS-SRVS 3.1.4.8).
//! Samba and many NAS devices reject the SMB SDK's NDR64-only RPC binding.

use anyhow::{Context, Result, bail, ensure};
use smb::{Client, Pipe};

const MAX_REPLY: usize = 8 * 1024 * 1024;
const SRVSVC: [u8; 20] = [
    0xc8, 0x4f, 0x32, 0x4b, 0x70, 0x16, 0xd3, 0x01, 0x12, 0x78, 0x5a, 0x47, 0xbf, 0x6e, 0xe1, 0x88,
    3, 0, 0, 0,
];
const NDR32: [u8; 20] = [
    0x04, 0x5d, 0x88, 0x8a, 0xeb, 0x1c, 0xc9, 0x11, 0x9f, 0xe8, 0x08, 0x00, 0x2b, 0x10, 0x48, 0x60,
    2, 0, 0, 0,
];

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn packet(kind: u8, call: u32, body: &[u8]) -> Vec<u8> {
    let mut packet = vec![5, 0, kind, 3, 0x10, 0, 0, 0];
    packet.extend_from_slice(&((16 + body.len()) as u16).to_le_bytes());
    packet.extend_from_slice(&0u16.to_le_bytes()); // No RPC auth trailer; SMB authenticates the pipe.
    packet.extend_from_slice(&call.to_le_bytes());
    packet.extend_from_slice(body);
    packet
}

fn bind_request() -> Vec<u8> {
    // One context, srvsvc v3.0, one transfer syntax: NDR32 v2.0.
    let mut body = vec![0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0];
    body.extend_from_slice(&SRVSVC);
    body.extend_from_slice(&NDR32);
    packet(11, 1, &body)
}

fn check_bind(body: &[u8]) -> Result<()> {
    let mut reader = Decoder::new(body);
    ensure!(
        reader.u16()? >= 24 && reader.u16()? >= 60,
        "Invalid RPC fragment limits"
    );
    reader.u32()?; // Association group.
    let address_len = reader.u16()? as usize;
    reader.take(address_len)?; // Secondary address may legitimately be empty.
    reader.align()?;
    ensure!(reader.take(4)?[0] == 1, "Expected one RPC bind result");
    let result = reader.u16()?;
    let reason = reader.u16()?;
    ensure!(
        result == 0,
        "Server rejected NDR32 share listing (result {result}, reason {reason})"
    );
    ensure!(reader.take(20)? == NDR32, "Unexpected RPC transfer syntax");
    Ok(())
}

/// Reassemble RPC fragments, including responses split across SMB reads.
async fn exchange(pipe: &Pipe, request: Vec<u8>, call: u32, kind: u8) -> Result<Vec<u8>> {
    ensure!(
        pipe.write_raw(&request).await? == request.len(),
        "Incomplete RPC pipe write"
    );
    let mut pending = Vec::new();
    let mut reply = Vec::new();
    let mut first = true;
    for _ in 0..4096 {
        while pending.len() < 16
            || pending.len() < u16::from_le_bytes([pending[8], pending[9]]) as usize
        {
            let data = pipe.read_raw(65536).await?;
            ensure!(
                !data.is_empty(),
                "SMB RPC pipe closed before response completed"
            );
            ensure!(
                pending.len() + data.len() <= MAX_REPLY,
                "RPC fragment too large"
            );
            pending.extend_from_slice(&data);
        }
        let length = u16::from_le_bytes([pending[8], pending[9]]) as usize;
        ensure!(
            length >= 16 && length <= pending.len(),
            "Invalid RPC fragment length"
        );
        let fragment: Vec<_> = pending.drain(..length).collect();
        ensure!(
            fragment[0..2] == [5, 0] && fragment[4..8] == [0x10, 0, 0, 0],
            "Unsupported RPC encoding"
        );
        ensure!(
            u32::from_le_bytes(fragment[12..16].try_into()?) == call,
            "Unexpected RPC call ID"
        );
        ensure!(
            fragment[10..12] == [0, 0],
            "Unexpected RPC authentication trailer"
        );
        if fragment[2] == 3 && fragment.len() >= 28 {
            let status = u32::from_le_bytes(fragment[24..28].try_into()?);
            bail!("SMB share-listing RPC fault 0x{status:08x}");
        }
        ensure!(
            fragment[2] == kind,
            "Unexpected RPC response type {}",
            fragment[2]
        );
        ensure!(
            (fragment[3] & 1 != 0) == first,
            "Invalid RPC fragment sequence"
        );
        let body = if kind == 2 {
            ensure!(
                length >= 24 && fragment[20..22] == [0, 0],
                "Invalid RPC response context"
            );
            &fragment[24..]
        } else {
            &fragment[16..]
        };
        ensure!(
            reply.len() + body.len() <= MAX_REPLY,
            "RPC response too large"
        );
        reply.extend_from_slice(body);
        if fragment[3] & 2 != 0 {
            ensure!(pending.is_empty(), "Unexpected data after RPC response");
            return Ok(reply);
        }
        first = false;
    }
    bail!("Too many RPC response fragments")
}

struct Decoder<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(size).context("RPC length overflow")?;
        let result = self
            .bytes
            .get(self.pos..end)
            .context("Truncated RPC response")?;
        self.pos = end;
        Ok(result)
    }
    fn align(&mut self) -> Result<()> {
        self.take((4 - self.pos % 4) % 4)?;
        Ok(())
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        self.align()?;
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn string(&mut self) -> Result<String> {
        let max = self.u32()?;
        let offset = self.u32()?;
        let count = self.u32()?;
        ensure!(
            offset == 0 && count > 0 && count <= max && count <= MAX_REPLY as u32 / 2,
            "Invalid NDR32 string bounds"
        );
        let data = self.take(count as usize * 2)?;
        let units: Vec<_> = data
            .as_chunks::<2>().0.iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        ensure!(units.last() == Some(&0), "Unterminated NDR32 string");
        let value = String::from_utf16(&units[..units.len() - 1])?;
        ensure!(!value.contains('\0'), "Invalid NDR32 string");
        Ok(value)
    }
}

fn decode_page(bytes: &[u8]) -> Result<(Vec<String>, Option<u32>)> {
    let mut reader = Decoder::new(bytes);
    ensure!(
        reader.u32()? == 1 && reader.u32()? == 1,
        "Unexpected share information level"
    );
    let mut shares = Vec::new();
    if reader.u32()? != 0 {
        let entries = reader.u32()? as usize;
        let buffer = reader.u32()?;
        ensure!(
            entries <= 65536 && (buffer != 0 || entries == 0),
            "Invalid share array"
        );
        if buffer != 0 {
            ensure!(
                reader.u32()? as usize == entries,
                "Inconsistent share array length"
            );
            let mut descriptors = Vec::new();
            for _ in 0..entries {
                descriptors.push((reader.u32()?, reader.u32()?, reader.u32()?));
            }
            for (name, kind, remark) in descriptors {
                let name = if name != 0 {
                    Some(reader.string()?)
                } else {
                    None
                };
                if remark != 0 {
                    reader.string()?;
                }
                if kind & 0xff == 0
                    && let Some(name) = name
                    && !matches!(name.as_str(), "" | "." | "..")
                    && !name.contains(['/', '\\', ':'])
                {
                    shares.push(name);
                }
            }
        }
    }
    reader.u32()?; // TotalEntries is advisory, not an allocation size.
    let resume = if reader.u32()? != 0 { reader.u32()? } else { 0 };
    let status = reader.u32()?;
    ensure!(
        reader.pos == bytes.len(),
        "Unexpected data after share enumeration"
    );
    match status {
        0 => Ok((shares, None)),
        234 if resume != 0 => Ok((shares, Some(resume))),
        5 => bail!(
            "Permission denied while listing SMB shares; enter /share in Start folder to connect directly"
        ),
        _ => bail!("SMB share listing failed (status {status})"),
    }
}

pub(super) async fn list_shares(client: &Client, server: &str) -> Result<Vec<String>> {
    let pipe = client.open_pipe(server, "srvsvc").await?;
    let result = async {
        check_bind(&exchange(&pipe, bind_request(), 1, 12).await?)?;
        let mut shares = Vec::new();
        let mut resume = 0;
        for call in 2..1026 {
            // Null ServerName denotes this server. Level 1, empty input container,
            // 4096-byte pages, and a non-null resume handle for pagination.
            let stub = words(&[0, 1, 1, 0x20000, 0, 0, 4096, 0x20004, resume]);
            let mut body = (stub.len() as u32).to_le_bytes().to_vec();
            body.extend_from_slice(&[0, 0, 15, 0]); // Context 0, NetrShareEnum opnum 15.
            body.extend_from_slice(&stub);
            let response = exchange(&pipe, packet(0, call, &body), call, 2).await?;
            let (page, next) = decode_page(&response)?;
            shares.extend(page);
            ensure!(shares.len() <= 65536, "Too many SMB shares");
            match next {
                None => {
                    shares.sort();
                    shares.dedup();
                    return Ok(shares);
                }
                Some(next) => {
                    ensure!(next != resume, "SMB share enumeration made no progress");
                    resume = next;
                }
            }
        }
        bail!("Too many SMB share enumeration pages")
    }
    .await;
    let close = pipe.close().await;
    let shares = result?;
    close?;
    Ok(shares)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(value: &str, data: &mut Vec<u8>) {
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        let units: Vec<_> = value.encode_utf16().chain([0]).collect();
        data.extend(words(&[units.len() as u32, 0, units.len() as u32]));
        data.extend(units.into_iter().flat_map(u16::to_le_bytes));
    }

    fn page(status: u32, resume: u32) -> Vec<u8> {
        let mut data = words(&[1, 1, 1, 2, 1, 2, 1, 0x80000000, 1, 1, 3, 0]);
        string("Données", &mut data);
        string("Disk share", &mut data);
        string("IPC$", &mut data);
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        data.extend(words(&[2, 1, resume, status]));
        data
    }

    #[test]
    fn decodes_unicode_disk_shares_and_resume_handles() {
        assert_eq!(
            decode_page(&page(0, 0)).unwrap(),
            (vec!["Données".into()], None)
        );
        assert_eq!(
            decode_page(&page(234, 7)).unwrap(),
            (vec!["Données".into()], Some(7))
        );
        assert!(decode_page(&page(234, 0)).is_err());
        assert!(
            decode_page(&page(5, 0))
                .unwrap_err()
                .to_string()
                .contains("Permission denied")
        );
        assert_eq!(
            decode_page(&words(&[1, 1, 0, 0, 0, 0])).unwrap(),
            (vec![], None)
        );
    }

    #[test]
    fn rejects_truncated_or_unbounded_network_data() {
        let data = page(0, 0);
        for length in 0..data.len() {
            assert!(decode_page(&data[..length]).is_err());
        }
        assert!(decode_page(&words(&[1, 1, 1, u32::MAX, 1, u32::MAX])).is_err());
        let mut invalid_string = words(&[u32::MAX, 0, u32::MAX]);
        assert!(Decoder::new(&invalid_string).string().is_err());
        invalid_string = words(&[2, 1, 2]);
        assert!(Decoder::new(&invalid_string).string().is_err());
        invalid_string = words(&[1, 0, 1]);
        invalid_string.extend_from_slice(&[65, 0]);
        assert!(Decoder::new(&invalid_string).string().is_err());
    }

    #[test]
    fn bind_ack_allows_empty_secondary_address_and_rejects_ndr32_rejection() {
        let mut body = vec![
            0xb8, 0x10, 0xb8, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0,
        ];
        body.extend_from_slice(&NDR32);
        check_bind(&body).unwrap();
        body[16] = 2;
        assert!(check_bind(&body).is_err());
    }
}
