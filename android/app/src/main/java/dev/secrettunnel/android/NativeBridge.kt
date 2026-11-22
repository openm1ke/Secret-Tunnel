package dev.secrettunnel.android

object NativeBridge {
    private val loadError = try {
        System.loadLibrary("secret_tunnel_android")
        null
    } catch (error: LinkageError) {
        error
    }

    internal fun requireAvailable() {
        loadError?.let { throw IllegalStateException("Не удалось загрузить SSH: ${it.message}", it) }
    }

    external fun start(config: String, service: TunnelVpnService): String
    external fun probe(config: String): String
    external fun checkServer(config: String): String
    external fun manageAgent(config: String, version: String, archivePath: String): String
    external fun generateKey(): String
    external fun registerKey(config: String, publicKey: String): String
    external fun enrollPassword(config: String, password: String): String
    external fun cancel()
    external fun stop()
    external fun status(): String
    external fun checkConnection(): String
    external fun trafficTotals(): LongArray
}
