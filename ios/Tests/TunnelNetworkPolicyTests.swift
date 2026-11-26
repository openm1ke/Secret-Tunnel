import XCTest
import NetworkExtension
@testable import SecretTunnel

final class TunnelNetworkPolicyTests: XCTestCase {
    func testBothDefaultRoutesArePresentEvenForAnIPv4SSHServer() throws {
        let settings = TunnelNetworkPolicy.networkSettings(server: "203.0.113.10")
        let ipv6 = try XCTUnwrap(settings.ipv6Settings)
        XCTAssertEqual(ipv6.addresses, [TunnelNetworkPolicy.ipv6Address])
        XCTAssertEqual(ipv6.includedRoutes?.count, 1)
        XCTAssertEqual(ipv6.includedRoutes?.first?.destinationAddress, "::")
        XCTAssertEqual(ipv6.includedRoutes?.first?.destinationNetworkPrefixLength, 0)
        XCTAssertTrue(ipv6.excludedRoutes?.isEmpty ?? true)
        XCTAssertEqual(settings.ipv4Settings?.includedRoutes?.first?.destinationAddress, "0.0.0.0")
        XCTAssertEqual(settings.dnsSettings?.servers, ["1.1.1.1"])
    }

    func testPacketEngineAndSystemAgreeOnIPv6AndLocalSocks() {
        let config = TunnelNetworkPolicy.hevConfiguration(socksPort: 10808, logPath: "/tmp/tun.log")
        XCTAssertTrue(config.contains("ipv6: '\(TunnelNetworkPolicy.ipv6Address)'"))
        XCTAssertTrue(config.contains("address: '::1'"))
        XCTAssertTrue(config.contains("port: 10808"))
        let proto = NETunnelProviderProtocol()
        TunnelNetworkPolicy.configure(proto)
        XCTAssertTrue(proto.includeAllNetworks)
        XCTAssertFalse(proto.excludeLocalNetworks)
    }

    func testNetworkExemptionNamesTheFirstSshHop() {
        XCTAssertEqual(TunnelNetworkPolicy.entryHost(target: "203.0.113.10", jumps: []), "203.0.113.10")
        XCTAssertEqual(TunnelNetworkPolicy.entryHost(target: "203.0.113.30", jumps: [["host": "203.0.113.10"], ["host": "203.0.113.20"]]), "203.0.113.10")
    }
}
