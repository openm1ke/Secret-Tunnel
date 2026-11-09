package dev.secrettunnel.android
import org.junit.Assert.*
import org.junit.Test
class RecoveryPolicyTest {
    @Test fun engineFailureDoesNotReconnectAHealthySsh(){
        val policy=RecoveryPolicy()
        assertEquals(RecoveryPolicy.Action.RESTART_ENGINE,policy.action(0,true,false))
        assertEquals(RecoveryPolicy.Action.RECONNECT_SSH,policy.action(0,false,false))
        policy.failed("temporary error",100)
        assertEquals(RecoveryPolicy.Action.WAIT,policy.action(100,true,false))
        assertEquals(RecoveryPolicy.Action.RESTART_ENGINE,policy.action(1100,true,false))
        assertEquals(RecoveryPolicy.Action.HEALTHY,policy.action(1200,true,true))
        assertEquals(RecoveryPolicy.Action.HEALTHY,policy.action(11200,true,true))
        assertEquals(0,policy.failures)
    }
    @Test fun aCrashLoopCannotResetRetryBudgetByBrieflyStarting(){
        val policy=RecoveryPolicy(4)
        repeat(4){
            policy.failed("engine stopped",it*30000L)
            policy.action(it*30000L+100,true,true) // transiently running, not stable
            policy.action(it*30000L+200,true,false)
        }
        assertTrue(policy.needsUser)
    }
    @Test fun terminalFailuresWaitForUserAndBackoffIsBounded(){
        val policy=RecoveryPolicy(5)
        repeat(5){policy.failed("socket closed",it*20000L)}
        assertTrue(policy.needsUser)
        assertEquals(RecoveryPolicy.Action.NEEDS_USER,policy.action(100000,false,false))
        policy.reset();policy.failed("Embedded SSH public key authentication failed",0)
        assertEquals(RecoveryPolicy.Action.NEEDS_USER,policy.action(100000,false,false))
        policy.reset();repeat(4){policy.failed("timeout",it*30000L);assertTrue(policy.nextAttemptAt-it*30000L<=15000)}
    }
    @Test fun aFailedAttemptIsCountedOnceAndTerminalErrorsStayLatched(){
        val policy=RecoveryPolicy()
        policy.failed("engine stopped",0)
        policy.failed("permission denied",100,countAttempt=false)
        assertEquals(1,policy.failures)
        assertTrue(policy.needsUser)
        policy.failed("status unavailable",200)
        assertTrue(policy.needsUser)
        assertEquals(RecoveryPolicy.Action.NEEDS_USER,policy.action(30000,false,false))
    }
    @Test fun agentInstallationErrorsWaitForUserButTimeoutsCanRecover(){
        for(code in listOf("AGENT_MISSING:","AGENT_NOT_EXECUTABLE:","AGENT_INCOMPATIBLE:","AGENT_INVALID_RESPONSE:","AGENT_RESOURCE_LIMIT:","AGENT_CHECK_FAILED:")){
            val policy=RecoveryPolicy();policy.failed(code,0);assertTrue(code,policy.needsUser)
        }
        val policy=RecoveryPolicy();policy.failed("AGENT_CHECK_TIMEOUT:",0);assertFalse(policy.needsUser)
    }
}
