# USB and Wi-Fi

After one connection by USB, **unplug the cable**: Mirrorly finds the iPhone
on your network and carries on over Wi-Fi. Plug it back in and it switches to
USB by itself. Nothing to set up: the Wi-Fi pairing happens during the first
USB connection.

| | 🔌 USB | 📶 Wi-Fi |
|---|---|---|
| Tap → on screen (median) | **87.5 ms** | 96.3 ms |
| Smoothness | steady | an occasional short stall on a busy network |
| Switching | plug in → back on USB in ~1.3 s | unplug → on Wi-Fi in ~2.1 s |

Most of the delay is the iPhone itself (noticing the touch, drawing,
encoding); Mirrorly's own share is about 1 ms. Measurements and method:
[LATENCY.md](https://github.com/AnasSAV/mirrorly/blob/main/LATENCY.md).

## Wi-Fi checklist

- PC and iPhone on the **same Wi-Fi**, iPhone unlocked.
- Windows' network profile set to **Private**: *Settings › Network &
  internet › Wi-Fi › (your network) › Network profile type › Private*. If
  Windows asks whether Mirrorly may use private networks, allow it.
- Not a guest, hotel or apartment network that keeps devices apart, and no
  full-tunnel VPN.
- For the smoothest picture: 5 GHz, and Wi-Fi power saving off (*Device
  Manager › Network adapters › your Wi-Fi adapter › Power Management*, untick
  *Allow the computer to turn off this device*).

An amber or red light at the top of the screen means the Wi-Fi link is
struggling.

Over Wi-Fi the connection is encrypted (TLS with a key from the pairing),
and the iPhone has to prove it is the one paired with this PC: it signs
with the key it gave when it was paired over USB, so another device on the
network can't stand in for it. Wi-Fi pairings made before Mirrorly 2.0
don't hold that key; such an iPhone renews its pairing the next time it is
plugged in, and isn't used over Wi-Fi until then.

To remove an iPhone's Wi-Fi pairing, disconnect and click **Forget** next
to it.
