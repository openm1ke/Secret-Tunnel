"""Check packaged ABIs, JNI dependencies, notices and 16 KiB ELF alignment."""
import struct
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1]) as apk:
    names = set(apk.namelist())
    assert 'AndroidManifest.xml' in names
    for abi, machine in (('armeabi-v7a', 40), ('arm64-v8a', 183), ('x86_64', 62)):
        for library in ('libsecret_tunnel_android.so', 'libhev-socks5-tunnel.so'):
            name = f'lib/{abi}/{library}'
            data = apk.read(name)
            entry = apk.getinfo(name)
            assert entry.compress_type == zipfile.ZIP_STORED, name
            with open(sys.argv[1], 'rb') as raw:
                raw.seek(entry.header_offset + 26)
                name_size, extra_size = struct.unpack('<HH', raw.read(4))
            assert (entry.header_offset + 30 + name_size + extra_size) % 16384 == 0, f'ZIP alignment: {name}'
            elf32 = abi == 'armeabi-v7a'
            assert data[:6] == b'\x7fELF' + bytes((1 if elf32 else 2, 1)), name
            assert struct.unpack_from('<H', data, 18)[0] == machine, name
            phoff = struct.unpack_from('<I' if elf32 else '<Q', data, 28 if elf32 else 32)[0]
            phsize, count = struct.unpack_from('<HH', data, 42 if elf32 else 54)
            layout = '<IIIIIIII' if elf32 else '<IIQQQQQQ'
            segments = [struct.unpack_from(layout, data, phoff + i*phsize) for i in range(count)]
            loadable = [segment for segment in segments if segment[0] == 1]
            assert loadable and all(segment[7] >= 16384 for segment in loadable), name
            print(f'PASS ELF 16 KiB: {name}')
            if library == 'libhev-socks5-tunnel.so':
                assert b'dev/secrettunnel/android/TunnelVpnService' in data, 'Wrong native JNI registration class'
                assert b'TProxyStartService\0' in data and b'(Ljava/lang/String;II)Z\0' in data, f'Wrong TProxyStartService JNI signature: {name}'
                for method in (b'TProxySetAccounting\0', b'TProxyDrainTraffic\0', b'isPriorityUdp\0'):
                    assert method in data, f'Missing packet engine JNI method {method!r}: {name}'
            else:
                for method in ('start', 'probe', 'generateKey', 'registerKey'):
                    assert ('Java_dev_secrettunnel_android_NativeBridge_' + method).encode() in data, f'Missing SSH JNI export: {method}'
    assert 'assets/licenses/hev-socks5-tunnel.txt' in names
    assert 'assets/licenses/rust-dependencies.json' in names
    assert apk.read('assets/hev-revision.txt').strip() == b'8e3b4cd7168fe64b64f8b58537b5623fb6a7074d'
    print('PASS APK ABIs, upstream pin and dependency notices')
