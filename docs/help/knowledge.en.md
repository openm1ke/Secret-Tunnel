# Secret Tunnel help

## Connection setup

You need your own Linux x86_64 server with SSH access. Open **Servers → Set up connection**. If you do not have a server yet, use **Server catalog** to compare locations and latency, then order from the provider directly.

Enter the server address, SSH port and username from your provider. Choose an existing private SSH key, or enter the server password so the wizard can create a separate key. The password is not saved.

Compare the displayed SSH host key fingerprint with your provider's control panel or ask your administrator. Only trust a matching fingerprint. The wizard then checks key authentication and offers to install the server agent.

## App selection

On macOS and Windows, select applications under **Apps** and enable interception after connecting the tunnel. Restart apps that already had open connections. Embedded SSH carries both TCP and UDP; system OpenSSH carries TCP only.

macOS requires ProxyBridge and its network extension. Nested helper apps may have their own signing identifiers. Windows interception requires the supported ProxyBridge/WinDivert component and administrator privileges.

On Android and Android TV, select applications under **Apps**, save your selection, and connect on **Home**. Approve Android's VPN request. Unselected apps use their regular connection. Reconnect after changing app selection.

On iPhone and iPad, the VPN routes the whole device. Selecting individual apps requires MDM management. The iOS simulator can check SSH and the agent, but cannot run the Packet Tunnel extension.

## Phone settings

Open **More → Settings** to change the language, theme, address privacy and connection checks. Language and theme changes do not restart the tunnel. The app reconnects automatically after network changes while the VPN is enabled.

Android's **Block if tunnel disconnects** option protects selected apps while Android keeps the VPN running. Stopping the VPN can restore direct internet access. Close protected apps before deliberately stopping the tunnel.

## Phone updates

Open **More → About** to check for updates. Downloads require your consent. Android verifies the APK checksum and signing identity before installation. Allow Secret Tunnel to install apps if Android requests it.

The server agent and the app must use matching versions. Update the server agent from **About**. The app downloads and verifies the archive locally, then sends it over SSH; the server does not need GitHub access.

## Server chain on phone

Create profiles for the intermediate and final SSH servers. In the final server's settings, choose the intermediate server under **Connect through**. Only the final server needs the agent. Verify every server's fingerprint and access key.

## Common issues

**Server does not respond:** check the address, SSH port and network. Compare Wi-Fi and mobile access. A server reachable from one network may be unavailable from another.

**Authentication failed:** check the SSH username and whether the matching public key is authorized on that account. A public .pub file is not a private key.

**Host key changed:** stop and verify the replacement with the provider or administrator. Do not approve an unexplained change.

**SSH works but traffic does not:** check the server agent version, app selection, interception or VPN permission. Connection checks test the local SOCKS path, not whether an application is intercepted.

**Content is slow:** compare a Quick speed test with and without the tunnel using the same service. Other VPNs, Wi-Fi signal, server bandwidth and route congestion can affect results. Technical logs preserve original diagnostics for troubleshooting.

## Share server settings

Open **Servers → Share** to create an encrypted link, QR code or .stprofile file. Share the transfer code separately. The transfer includes private access keys: only share it with someone you trust. Existing server profiles on the receiving device are preserved.

On Android TV, copy the .stprofile file to a USB drive and choose **Servers → Import → Open file**.

## Connection profiles

**Balanced** is the default traffic profile. **Voice** prioritizes calls. The optional second SSH connection separates selected apps' UDP traffic from background traffic; it does not guarantee higher throughput or eliminate congestion.

UDP windows and SSH queues are advanced settings. Larger windows can improve distant routes but use more memory. Start with defaults and compare measurements before changing them.

## Privacy and DNS

Selected applications' tunneled DNS requests are resolved from the server side. Application selection and system DNS are different mechanisms; connection checks do not establish that all device DNS uses the tunnel. Other VPNs and secure DNS in browsers can change the path.

IPv6 traffic covered by the VPN must either use the tunnel or be blocked when the server cannot carry it. Do not disable protection to work around connection errors without understanding the resulting route.

## Server catalog

Flags identify server locations. Latency is the median of five TCP connections from your current network, not ICMP ping. A provider's test endpoint may differ from the VPS you purchase. Advertised port speed is not a guarantee of tunnel speed. Check the final price, taxes, IPv4 cost and traffic allowance with the provider.
