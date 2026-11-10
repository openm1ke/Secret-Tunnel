package dev.secrettunnel.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class SshRouteTest {
    @Test fun trafficPriorityDefaultsAndValidationAreExplicit() {
        assertEquals("balanced", SshSettings().qos)
        assertEquals("voice", SshSettings(qos="voice").copy().qos)
        assertEquals("throughput", SshSettings(qos="throughput").qos)
        assertThrows(IllegalArgumentException::class.java) { SshSettings(qos="unknown") }
    }

    private fun server(id: String, jump: String? = null) =
        ServerProfile(id, id, "$id.example", 22, "test", jump)

    @Test fun resolvesDirectAndChainedServersInConnectionOrder() {
        val first = server("first")
        val second = server("second", "first")
        val exit = server("exit", "second")
        assertEquals(listOf(first), resolveServerRoute(first.id, listOf(first)))
        assertEquals(listOf(first, second, exit), resolveServerRoute(exit.id, listOf(exit, first, second)))
    }

    @Test fun rejectsCyclesAndMissingServers() {
        assertThrows(IllegalArgumentException::class.java) {
            resolveServerRoute("a", listOf(server("a", "b"), server("b", "a")))
        }
        assertThrows(IllegalStateException::class.java) {
            resolveServerRoute("a", listOf(server("a", "missing")))
        }
    }
}
