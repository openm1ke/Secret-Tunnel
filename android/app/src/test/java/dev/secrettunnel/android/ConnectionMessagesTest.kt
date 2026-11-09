package dev.secrettunnel.android

import org.junit.Assert.*
import org.junit.Test

class ConnectionMessagesTest {
    @Test fun agentTimeoutPreservesTheSuccessfulSshStage() {
        val message = ConnectionMessages.describe("AGENT_CHECK_TIMEOUT: SSH и ключ проверены; агент не ответил за 4 секунды")
        assertTrue(message.startsWith("SSH и ключ проверены"))
        assertTrue(message.contains("агент"))
        assertFalse(message.contains("AGENT_CHECK_TIMEOUT"))
        assertTrue(ConnectionMessages.describe("SSH connection timed out").startsWith("Сервер не ответил"))
    }
    @Test fun authenticationTrustAndUnknownErrorsStayDistinct() {
        assertTrue(ConnectionMessages.describe("public key authentication failed").contains("не принял SSH-ключ"))
        assertTrue(ConnectionMessages.describe("Unknown server key").contains("подтвердите ключ сервера"))
        assertEquals(240,ConnectionMessages.describe("x".repeat(1000)).length)
    }
}
