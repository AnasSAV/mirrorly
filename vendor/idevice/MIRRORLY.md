# idevice 0.1.68, with Mirrorly's changes

This is [idevice](https://github.com/jkcoxson/idevice) 0.1.68 by Jackson
Coxson (MIT, `LICENSE.txt`), as published on crates.io, with one change.
The root `Cargo.toml` uses it through `[patch.crates-io]`.

## Why

Over Wi-Fi, Mirrorly connects with RemotePairing pair-verify. idevice
checked only one side of it: the PC proved itself to the phone, but the
phone never proved itself to the PC. In 0.1.68, `validate_pairing` used only
the public key in the phone's M2 message and never looked at its encrypted
part, and pair-setup kept no key of the phone's to check it against.

So anything on the network that announced itself with the phone's Bonjour
tag (which the phone broadcasts) could finish the handshake and be taken
for the paired phone. Mirrorly would then send it the PC's clipboard and
keystrokes.

## What changed (`src/remote_pairing/`)

- **`rp_pairing_file.rs`:** `RpPairingFile` gains `device_identifier` and
  `device_public_key`. They are saved in the record, and are optional, so
  older records still load.
- **`mod.rs`, `pair`:** keeps the device's Identifier and Ed25519
  PublicKey from its pair-setup M6. It already decrypted M6; before this
  change it read only `altIRK` from it.
- **`mod.rs`, `validate_pairing`:** decrypts M2's EncryptedData (nonce
  `PV-Msg02`, with the session key the PC derives anyway). It then checks
  the device's Ed25519 signature over *device X25519 key ‖ device
  identifier ‖ host X25519 key*, and that the identifier is the one from
  pair-setup. When the record holds the device's key and the check fails,
  it stops with `RemotePairingError::DeviceNotVerified` before answering.
  `device_verified()` reports the result.
- **`errors.rs`:** adds `DeviceNotVerified` (sub-code 8).
- **Tests:** `mirrorly_device_check` at the end of `mod.rs`.

This is HomeKit's pair-verify, which RemotePairing follows: the same
salts, info strings and nonces. It was confirmed on an iPhone 11 on iOS 27
(2026-11-20). M6 carries Identifier (36 bytes), PublicKey (32) and
Signature (64), and the M2 signature verifies with that key.

pymobiledevice3 skips this check too: its `_validate_pairing` has a TODO
saying it couldn't verify the encrypted data. This change could go
upstream.

Run the tests with:

```
cargo test --manifest-path vendor/idevice/Cargo.toml --features remote_pairing --lib mirrorly_device_check
```
