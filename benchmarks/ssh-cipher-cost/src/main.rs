use ring::aead::{self, chacha20_poly1305_openssh as chacha, Aad, LessSafeKey, Nonce, UnboundKey};
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

// Public synthetic keys only. No application credentials or network access.
fn measure(name: &str, size: usize, round: usize) {
    let target = Duration::from_millis(250);
    let mut count = 0u32;
    let start = Instant::now();
    if name == "chacha20-poly1305@openssh.com" {
        let material = [round as u8 + 1; chacha::KEY_LEN];
        let seal = chacha::SealingKey::new(&material);
        let open = chacha::OpeningKey::new(&material);
        let mut packet = vec![0x63; size + 4];
        let mut tag = [0; chacha::TAG_LEN];
        while start.elapsed() < target {
            for _ in 0..128 {
                packet[..4].copy_from_slice(&(size as u32).to_be_bytes());
                seal.seal_in_place(count, &mut packet, &mut tag);
                let _length =
                    black_box(open.decrypt_packet_length(count, packet[..4].try_into().unwrap()));
                black_box(open.open_in_place(count, &mut packet, &tag).unwrap());
                count += 1;
            }
        }
    } else {
        let algorithm = if name == "aes128-gcm@openssh.com" {
            &aead::AES_128_GCM
        } else {
            &aead::AES_256_GCM
        };
        let material = vec![round as u8 + 1; algorithm.key_len()];
        let seal = LessSafeKey::new(UnboundKey::new(algorithm, &material).unwrap());
        let open = LessSafeKey::new(UnboundKey::new(algorithm, &material).unwrap());
        let mut packet = vec![0x63; size + 16];
        let aad = (size as u32).to_be_bytes();
        while start.elapsed() < target {
            for _ in 0..128 {
                let mut nonce = [0; 12];
                nonce[8..].copy_from_slice(&count.to_be_bytes());
                let tag = seal
                    .seal_in_place_separate_tag(
                        Nonce::assume_unique_for_key(nonce),
                        Aad::from(&aad),
                        &mut packet[..size],
                    )
                    .unwrap();
                packet[size..].copy_from_slice(tag.as_ref());
                black_box(
                    open.open_in_place(
                        Nonce::assume_unique_for_key(nonce),
                        Aad::from(&aad),
                        &mut packet,
                    )
                    .unwrap(),
                );
                count += 1;
            }
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!("{{\"round\":{round},\"cipher\":\"{name}\",\"payload_bytes\":{size},\"roundtrips\":{count},\"seconds\":{elapsed:.6},\"seal_open_ns\":{ns:.2},\"processed_mib_per_s\":{speed:.2}}}",
        ns=elapsed*1e9/count as f64,speed=2.*size as f64*count as f64/elapsed/1048576.);
}

fn main() {
    let ciphers = [
        "chacha20-poly1305@openssh.com",
        "aes128-gcm@openssh.com",
        "aes256-gcm@openssh.com",
    ];
    for round in 0..3 {
        for size in [160, 1200, 16384] {
            for index in 0..3 {
                measure(ciphers[(index + round) % 3], size, round);
            }
        }
    }
}
