package dev.secrettunnel.android

object NativeBridge {
    init { System.loadLibrary("secret_tunnel_android") }
    external fun start(config: String, service: TunnelVpnService): String
    external fun probe(host: String, port: Int): String
    external fun generateKey(): String
    external fun registerKey(config: String, publicKey: String): String
    external fun cancel()
    external fun stop()
    external fun status(): String
}
