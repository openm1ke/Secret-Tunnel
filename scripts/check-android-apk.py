"""Check packaged ABIs, JNI dependencies, notices and 16 KiB ELF alignment."""
import struct
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1]) as apk:
    names = set(apk.namelist())
    assert 'AndroidManifest.xml' in names
    for abi in ('arm64-v8a', 'x86_64'):
        for library in ('libsecret_tunnel_android.so', 'libhev-socks5-tunnel.so'):
            name = f'lib/{abi}/{library}'
            data = apk.read(name)
            entry = apk.getinfo(name)
            assert entry.compress_type == zipfile.ZIP_STORED, name
            with open(sys.argv[1], 'rb') as raw:
                raw.seek(entry.header_offset + 26)
                name_size, extra_size = struct.unpack('<HH', raw.read(4))
            assert (entry.header_offset + 30 + name_size + extra_size) % 16384 == 0, f'ZIP alignment: {name}'
            assert data[:6] == b'\x7fELF\x02\x01', name
            phoff = struct.unpack_from('<Q', data, 32)[0]
            phsize, count = struct.unpack_from('<HH', data, 54)
            segments = [struct.unpack_from('<IIQQQQQQ', data, phoff + i*phsize) for i in range(count)]
            assert all(segment[7] >= 16384 for segment in segments if segment[0] == 1), name
            print(f'PASS ELF 16 KiB: {name}')
            if library == 'libhev-socks5-tunnel.so':
                assert b'dev/secrettunnel/android/TunnelVpnService' in data, 'Wrong native JNI registration class'
            else:
                for method in ('start', 'probe', 'generateKey', 'registerKey'):
                    assert ('Java_dev_secrettunnel_android_NativeBridge_' + method).encode() in data, f'Missing SSH JNI export: {method}'
    assert 'assets/licenses/hev-socks5-tunnel.txt' in names
    assert 'assets/licenses/rust-dependencies.json' in names
    assert apk.read('assets/hev-revision.txt').strip() == b'8e3b4cd7168fe64b64f8b58537b5623fb6a7074d'
    print('PASS APK ABIs, upstream pin and dependency notices')
