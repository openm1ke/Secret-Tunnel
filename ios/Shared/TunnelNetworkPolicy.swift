import Foundation
import NetworkExtension

/// Both families stay inside utun even when the exit has no IPv6. The Rust
/// SOCKS endpoint rejects unavailable IPv6; omitting its route would leak it.
enum TunnelNetworkPolicy {
    static let ipv6Address = "fd00:198:18::1"

    static func entryHost(target: String, jumps: [[String: Any]]) -> String {
        (jumps.first?["host"] as? String) ?? target
    }

    static func networkSettings(server: String) -> NEPacketTunnelNetworkSettings {
        let network = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: server)
        let ipv4 = NEIPv4Settings(addresses: ["198.18.0.1"], subnetMasks: ["255.255.255.252"])
        ipv4.includedRoutes = [NEIPv4Route.default()]
        network.ipv4Settings = ipv4
        let ipv6 = NEIPv6Settings(addresses: [ipv6Address], networkPrefixLengths: [126])
        ipv6.includedRoutes = [NEIPv6Route.default()]
        network.ipv6Settings = ipv6
        let dns = NEDNSSettings(servers: ["1.1.1.1"])
        dns.matchDomains = [""]
        network.dnsSettings = dns
        network.mtu = 1500
        return network
    }

    static func configure(_ proto: NETunnelProviderProtocol) {
        // enforceRoutes is for split-tunnel routes; full default routes need
        // includeAllNetworks to prevent an app from selecting a physical link.
        proto.includeAllNetworks = true
        proto.excludeLocalNetworks = false
    }

    static func hevConfiguration(socksPort: Int, logPath: String) -> String {
        """
        tunnel:
          mtu: 1500
          ipv4: 198.18.0.1
          ipv6: '\(ipv6Address)'
        socks5:
          address: '::1'
          port: \(socksPort)
          udp: 'udp'
        misc:
          task-stack-size: 65536
          tcp-buffer-size: 65536
          max-session-count: 1024
          connect-timeout: 14000
          tcp-read-write-timeout: 60000
          udp-read-write-timeout: 30000
          log-file: '\(logPath)'
          log-level: info
        """
    }
}
