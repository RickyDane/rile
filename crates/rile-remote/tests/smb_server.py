"""Loopback-only SMB fixture. Requires `pip install impacket` in a test venv."""

import sys

from impacket import ntlm, smb3structs, smbserver
from impacket.dcerpc.v5 import srvs


original_write = smbserver.SMB2Commands.smb2Write


def write_response(*args):
    commands, packets, status = original_write(*args)
    # Impacket 0.13.1 sets this reserved field to 0xff, contrary to MS-SMB2
    # 2.2.22 (MUST be zero). Keep the fixture wire-compatible with real servers.
    for command in commands or []:
        if isinstance(command, smb3structs.SMB2Write_Response):
            command["Remaining"] = 0
    return commands, packets, status


smbserver.SMB2Commands.smb2Write = staticmethod(write_response)
original_create = smbserver.SMB2Commands.smb2Create


def create_response(*args):
    commands, packets, status = original_create(*args)
    # Impacket echoes FILE_OVERWRITE_IF (5) as CreateAction; MS-SMB2 2.2.14
    # only allows 0..3 here. The backend does not depend on this action value.
    for command in commands or []:
        if isinstance(command, smb3structs.SMB2Create_Response) and command["CreateAction"] == 5:
            command["CreateAction"] = 3
    return commands, packets, status


smbserver.SMB2Commands.smb2Create = staticmethod(create_response)

# Force one entry per page to exercise ResumeHandle and ERROR_MORE_DATA,
# including pages containing only IPC$ (which must not appear as a disk share).
original_enum = smbserver.SRVSServer.NetrShareEnum


def share_enum(self, data):
    request = srvs.NetrShareEnum(data)
    answer = original_enum(self, data)
    container = answer["InfoStruct"]["ShareInfo"]["Level1"]
    entries = list(container["Buffer"])
    # Impacket assigns configuration strings to an NDR integer, silently
    # serializing every share type as zero. Preserve the configured wire type.
    for entry in entries:
        name = entry["shi1_netname"].rstrip("\x00")
        entry["shi1_type"] = int(self._shares[name]["share type"])
    start = int(request["ResumeHandle"])
    page = entries[start:start + 1]
    container["EntriesRead"] = len(page)
    container["Buffer"] = page
    next_entry = start + len(page)
    answer["ResumeHandle"] = next_entry if next_entry < len(entries) else 0
    answer["ErrorCode"] = 234 if next_entry < len(entries) else 0
    return answer


smbserver.SRVSServer.NetrShareEnum = share_enum
server = smbserver.SimpleSMBServer(listenAddress="127.0.0.1", listenPort=int(sys.argv[1]))
server.setSMB2Support(True)
server.addShare(sys.argv[2], sys.argv[3])
server.addShare("Second", sys.argv[3])
server.addCredential("alice", 0, ntlm.compute_lmhash("s3cret"), ntlm.compute_nthash("s3cret"))
server.start()
