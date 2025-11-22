package ai.lightcode.core.engine

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import org.junit.jupiter.api.AfterAll
import org.junit.jupiter.api.BeforeAll
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.TestInstance
import org.junit.jupiter.api.Assumptions.assumeTrue
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue
import kotlin.test.fail

@TestInstance(TestInstance.Lifecycle.PER_CLASS)
class CoreEngineApiTest {

    private val json = Json

    @BeforeAll
    fun setUp() {
        assumeTrue(NativeTestSupport.isNativeAvailable(), "codex_core_jni shared library missing; skipping")
        CoreEngineApi.initialize()
    }

    @AfterAll
    fun tearDown() {
        runCatching { CoreEngineApi.shutdown() }
    }

    @Test
    fun echoRoundTripsPayload() {
        val payload = buildJsonObject {
            put("message", "hello world")
            put("count", 42)
        }

        val response = CoreEngineApi.echo(payload)
        assertEquals("ok", response.status)
        assertEquals("echo", response.kind)
        assertEquals(payload, response.payload)
    }

    @Test
    fun parseIdTokenExtractsEmailAndPlan() {
        val response = CoreEngineApi.parseIdToken(SAMPLE_ID_TOKEN)
        assertEquals("ok", response.status)
        assertEquals("parsed_id_token", response.kind)
        assertEquals("dev@openai.com", response.email)
        assertEquals("Pro", response.chatgptPlanType)
    }

    @Test
    fun autoDriveCountdownTickRefreshesWhileTimeRemains() {
        val response = CoreEngineApi.autoDriveCountdownTick(
            phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
            countdownId = 17,
            decisionSeq = 4,
            secondsLeft = 5,
        )

        assertEquals("ok", response.status)
        assertEquals("auto_drive_countdown_tick", response.kind)
        assertEquals(1, response.effects.size)
        assertEquals("refresh_ui", response.effects.first().type)
        assertEquals(5, response.secondsLeft)
    }

    @Test
    fun autoDriveCountdownTickSubmitsWhenTimerExpires() {
        val response = CoreEngineApi.autoDriveCountdownTick(
            phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
            countdownId = 99,
            decisionSeq = 8,
            secondsLeft = 0,
        )

        assertEquals("ok", response.status)
        assertEquals("auto_drive_countdown_tick", response.kind)
        assertEquals(1, response.effects.size)
        assertEquals("submit_prompt", response.effects.first().type)
        assertEquals(0, response.secondsLeft)
    }

    @Test
    fun kotlinCountdownMirrorMatchesRust() {
        val scenarios = listOf(
            AutoDriveCountdownTickRequest(
                phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
                countdownId = 3,
                decisionSeq = 1,
                secondsLeft = 5,
            ),
            AutoDriveCountdownTickRequest(
                phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
                countdownId = 4,
                decisionSeq = 2,
                secondsLeft = 0,
            ),
            AutoDriveCountdownTickRequest(
                phase = AutoRunPhasePayload.active(),
                countdownId = 5,
                decisionSeq = 3,
                secondsLeft = 7,
            ),
            AutoDriveCountdownTickRequest(
                phase = AutoRunPhasePayload.pausedManual(resumeAfterSubmit = true, bypassNextSubmit = false),
                countdownId = 6,
                decisionSeq = 4,
                secondsLeft = 2,
            ),
        )

        scenarios.forEach { scenario ->
            val kotlinResult = AutoDriveControllerKt.handleCountdownTick(scenario)
            val rustResult = CoreEngineApi.autoDriveCountdownTick(
                phase = scenario.phase,
                countdownId = scenario.countdownId,
                decisionSeq = scenario.decisionSeq,
                secondsLeft = scenario.secondsLeft,
            )

            assertEquals(kotlinResult, rustResult, "Countdown parity failed for $scenario")
        }
    }

    @Test
    fun kotlinUpdateContinueModeMatchesRust() {
        val scenarios = listOf(
            AutoDriveUpdateContinueModeRequest(
                phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
                continueMode = AutoContinueModePayload.TEN_SECONDS,
                countdownId = 12,
                decisionSeq = 9,
            ),
            AutoDriveUpdateContinueModeRequest(
                phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
                continueMode = AutoContinueModePayload.IMMEDIATE,
                countdownId = 42,
                decisionSeq = 5,
            ),
            AutoDriveUpdateContinueModeRequest(
                phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = false),
                continueMode = AutoContinueModePayload.SIXTY_SECONDS,
                countdownId = 4,
                decisionSeq = 2,
            ),
            AutoDriveUpdateContinueModeRequest(
                phase = AutoRunPhasePayload.pausedManual(resumeAfterSubmit = true, bypassNextSubmit = false),
                continueMode = AutoContinueModePayload.MANUAL,
                countdownId = 7,
                decisionSeq = 3,
            ),
        )

        scenarios.forEach { scenario ->
            val kotlinResult = AutoDriveControllerKt.updateContinueMode(scenario)
            val rustResult = CoreEngineApi.autoDriveUpdateContinueMode(
                phase = scenario.phase,
                continueMode = scenario.continueMode,
                countdownId = scenario.countdownId,
                decisionSeq = scenario.decisionSeq,
            )

            assertEquals(kotlinResult, rustResult, "Continue mode parity failed for $scenario")
        }
    }

    @Test
    fun autoDriveSequenceMatchesRust() {
        val initialState = AutoDriveControllerStatePayload(
            phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
            continueMode = AutoContinueModePayload.TEN_SECONDS,
            countdownId = 10,
            countdownDecisionSeq = 4,
        )
        val operations = listOf(
            AutoDriveSequenceOperationPayload.UpdateContinueMode(AutoContinueModePayload.SIXTY_SECONDS),
            AutoDriveSequenceOperationPayload.HandleCountdownTick(countdownId = 11, decisionSeq = 4, secondsLeft = 5),
            AutoDriveSequenceOperationPayload.HandleCountdownTick(countdownId = 11, decisionSeq = 4, secondsLeft = 0),
            AutoDriveSequenceOperationPayload.PauseForTransientFailure("network hiccup"),
            AutoDriveSequenceOperationPayload.UpdateContinueMode(AutoContinueModePayload.MANUAL),
        )
        assertSequenceMatchesRust(initialState, operations)
    }

    @Test
    fun kotlinControllerEmitsExecRequestAfterLaunch() {
        val controller = KotlinAutoDriveControllerState(
            AutoDriveControllerStatePayload(
                phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
                continueMode = AutoContinueModePayload.MANUAL,
                countdownId = 1,
                countdownDecisionSeq = 1,
            ),
        )
        val operations = listOf(
            AutoDriveSequenceOperationPayload.UpdateContinueMode(AutoContinueModePayload.TEN_SECONDS),
            AutoDriveSequenceOperationPayload.HandleCountdownTick(
                countdownId = 1,
                decisionSeq = 1,
                secondsLeft = 0,
            ),
            AutoDriveSequenceOperationPayload.LaunchResult(
                result = AutoDriveSequenceOperationPayload.LaunchOutcome.SUCCEEDED,
                goal = "Write Kotlin exec test",
                error = null,
            ),
        )
        val steps = operations.map { controller.apply(it) }
        val execEffect = steps.flatMap { it.effects }.firstOrNull { it.execRequest != null }
        val execRequest = assertNotNull(execEffect?.execRequest, "expected Kotlin exec request effect")
        assertEquals(
            listOf("bash", "-lc", "echo Kotlin exec pipeline ready"),
            execRequest.command,
        )
    }

    @Test
    fun kotlinControllerEmitsPatchRequestAfterStop() {
        val controller = KotlinAutoDriveControllerState(
            AutoDriveControllerStatePayload(
                phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
                continueMode = AutoContinueModePayload.MANUAL,
                countdownId = 1,
                countdownDecisionSeq = 1,
            ),
        )
        val operations = listOf(
            AutoDriveSequenceOperationPayload.LaunchResult(
                result = AutoDriveSequenceOperationPayload.LaunchOutcome.SUCCEEDED,
                goal = "Capture Kotlin patch",
                error = null,
            ),
            AutoDriveSequenceOperationPayload.StopRun(message = "Done"),
        )
        val steps = operations.map { controller.apply(it) }
        val patchEffect = steps.flatMap { it.effects }.firstOrNull { it.patchRequest != null }
        val patchRequest = assertNotNull(patchEffect?.patchRequest, "expected Kotlin patch request")
        val change = patchRequest.changes["KOTLIN_PATCH_NOTE.md"]
        assertEquals(AutoDrivePatchChangeKind.Add, change?.kind)
        assertTrue(change?.content?.contains("Kotlin Patch Note") == true)
    }

    @Test
    fun autoDriveSequenceStopsAfterMaxTransientFailures() {
        val initialState = AutoDriveControllerStatePayload(
            phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
            continueMode = AutoContinueModePayload.SIXTY_SECONDS,
            countdownId = 7,
            countdownDecisionSeq = 2,
        )
        val operations = List(AUTO_RESTART_MAX_ATTEMPTS + 1) { attempt ->
            AutoDriveSequenceOperationPayload.PauseForTransientFailure(
                reason = "unstable network #${attempt + 1}",
            )
        }

        assertSequenceMatchesRust(initialState, operations)
    }

    @Test
    fun autoDriveSequenceIgnoresCountdownsOutsideAwaitingCoordinator() {
        val initialState = AutoDriveControllerStatePayload(
            phase = AutoRunPhasePayload.active(),
            continueMode = AutoContinueModePayload.MANUAL,
            countdownId = 3,
            countdownDecisionSeq = 8,
        )
        val operations = listOf(
            AutoDriveSequenceOperationPayload.HandleCountdownTick(countdownId = 3, decisionSeq = 8, secondsLeft = 4),
            AutoDriveSequenceOperationPayload.UpdateContinueMode(AutoContinueModePayload.SIXTY_SECONDS),
            AutoDriveSequenceOperationPayload.HandleCountdownTick(countdownId = 3, decisionSeq = 8, secondsLeft = 0),
            AutoDriveSequenceOperationPayload.LaunchResult(
                result = AutoDriveSequenceOperationPayload.LaunchOutcome.SUCCEEDED,
                goal = "Ship feature",
            ),
            AutoDriveSequenceOperationPayload.StopRun(message = "Done"),
        )

        assertSequenceMatchesRust(initialState, operations)
    }

    @Test
    fun conversationPruneHistoryMatchesRust() {
        val history = sampleHistory()

        val kotlinResult = CoreEngineFacade.Conversation.pruneHistoryLocally(history, dropLastUserTurns = 1)
        val rustResult = CoreEngineFacade.Conversation.pruneHistoryViaRust(history, dropLastUserTurns = 1)

        assertEquals(kotlinResult, rustResult)
    }

    @Test
    fun conversationPruneHistoryResetsWhenDroppingTooManyTurns() {
        val history = sampleHistory()

        val kotlinResult = CoreEngineFacade.Conversation.pruneHistoryLocally(history, dropLastUserTurns = 4)
        val rustResult = CoreEngineFacade.Conversation.pruneHistoryViaRust(history, dropLastUserTurns = 4)

        assertEquals(kotlinResult, rustResult)
        assertEquals(true, rustResult.wasReset)
    }

    @Test
    fun conversationPruneHistoryNoopWhenDroppingZero() {
        val history = sampleHistory()

        val kotlinResult = CoreEngineFacade.Conversation.pruneHistoryLocally(history, dropLastUserTurns = 0)
        val rustResult = CoreEngineFacade.Conversation.pruneHistoryViaRust(history, dropLastUserTurns = 0)

        assertEquals(kotlinResult, rustResult)
    }

    @Test
    fun conversationFilterHistoryMatchesRust() {
        val history = sampleHistory()

        val kotlinResult = CoreEngineFacade.Conversation.filterHistoryLocally(history)
        val rustResult = CoreEngineFacade.Conversation.filterHistoryViaRust(history)

        assertEquals(kotlinResult, rustResult)
    }

    @Test
    fun conversationFilterHistoryDropsSystemAndOther() {
        val history = sampleHistory()

        val rustResult = CoreEngineApi.conversationFilterHistory(history)
        assertEquals(2, rustResult.removedCount)
        val firstType = (rustResult.history.first() as JsonObject)["type"]?.jsonPrimitive?.content
        assertEquals("message", firstType)
    }

    @Test
    fun conversationSnapshotCoalesceMatchesRust() {
        val records = sampleSnapshotRecords()

        val kotlinResult = CoreEngineFacade.Conversation.coalesceSnapshotLocally(records)
        val rustResult = CoreEngineFacade.Conversation.coalesceSnapshotViaRust(records)

        assertEquals(kotlinResult, rustResult)
    }

    @Test
    fun conversationSnapshotCoalesceDropsDuplicateStreams() {
        val records = sampleSnapshotRecords()

        val rustResult = CoreEngineFacade.Conversation.coalesceSnapshotViaRust(records)
        assertEquals(1, rustResult.removedCount)
    }

    @Test
    fun conversationSnapshotSummaryMatchesRust() {
        val records = sampleSnapshotRecords()
        val kotlinResult = CoreEngineFacade.Conversation.snapshotSummaryLocally(records)
        val rustResult = CoreEngineFacade.Conversation.snapshotSummaryViaRust(records)

        assertEquals(kotlinResult, rustResult)
    }

    @Test
    fun conversationForkHistoryMatchesRust() {
        val history = sampleHistory()
        val kotlinResult = CoreEngineFacade.Conversation.forkHistoryLocally(history, dropLastUserTurns = 1)
        val rustResult = CoreEngineFacade.Conversation.forkHistoryViaRust(history, dropLastUserTurns = 1)

        assertEquals(kotlinResult, rustResult)
    }

    @Test
    fun conversationPopularCommandsFilterMatchesRust() {
        val history = sampleHistory()
        val kotlinResult = CoreEngineFacade.Conversation.filterPopularCommandsLocally(history)
        val rustResult = CoreEngineFacade.Conversation.filterPopularCommandsViaRust(history)

        assertEquals(kotlinResult, rustResult)
    }

    @Test
    fun plannerSeedMatchesRustWithoutAgents() {
        val goal = "Refactor diagnostics mode"
        val kotlinResult = CoreEngineFacade.Conversation.planningSeedLocally(goal, includeAgents = false)
        val rustResult = CoreEngineFacade.Conversation.planningSeedViaRust(goal, includeAgents = false)

        assertEquals(rustResult, kotlinResult)
        assertEquals("auto_coordinator_planning_seed", rustResult.kind)
        assertEquals(null, rustResult.agentsTiming)
    }

    @Test
    fun plannerSeedMatchesRustWithAgents() {
        val goal = "Launch multi-agent refactor"
        val kotlinResult = CoreEngineFacade.Conversation.planningSeedLocally(goal, includeAgents = true)
        val rustResult = CoreEngineFacade.Conversation.planningSeedViaRust(goal, includeAgents = true)

        assertEquals(rustResult, kotlinResult)
        assertEquals(AutoTurnAgentsTimingPayload.PARALLEL, rustResult.agentsTiming)
        requireNotNull(rustResult.responseJson)
        requireNotNull(rustResult.cliPrompt)
    }

    @Test
    fun autoDriveTurnFlowMatchesRust() {
        val request = sampleTurnFlowRequest()
        val result = TurnFlowOrchestrator.run(request)

        assertEquals(result.autoSequence.local, result.autoSequence.rust)
        assertEquals(result.pruneHistory.local, result.pruneHistory.rust)
        assertEquals(result.filterHistory.local, result.filterHistory.rust)
        assertEquals(result.coalesceSnapshot.local, result.coalesceSnapshot.rust)
    }

    companion object {
        private const val SAMPLE_ID_TOKEN = "eyJhbGciOiJIUzI1NiJ9.eyJlbWFpbCI6ImRldkBvcGVuYWkuY29tIiwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfcGxhbl90eXBlIjoicHJvIn19.signature"
    }

    private fun assertSequenceMatchesRust(
        initialState: AutoDriveControllerStatePayload,
        operations: List<AutoDriveSequenceOperationPayload>,
    ) {
        val kotlinResponse = CoreEngineFacade.AutoDrive.runSequenceLocally(initialState, operations)
        val rustResponse = CoreEngineFacade.AutoDrive.runSequenceViaRust(initialState, operations)

        assertEquals("ok", rustResponse.status)
        assertEquals("auto_drive_sequence", rustResponse.kind)
        assertSequenceParity(kotlinResponse.steps, rustResponse.steps)
    }

    private fun assertSequenceParity(
        expected: List<AutoDriveSequenceStepPayload>,
        actual: List<AutoDriveSequenceStepPayload>,
    ) {
        val normalizedExpected = expected.map { it.copy(effects = stripKotlinOnlyEffects(it.effects)) }
        val normalizedActual = actual.map { it.copy(effects = stripKotlinOnlyEffects(it.effects)) }

        if (normalizedExpected == normalizedActual) {
            return
        }

        val mismatchDescription = describeSequenceMismatch(normalizedExpected, normalizedActual)
        fail(mismatchDescription)
    }

    private fun stripKotlinOnlyEffects(effects: List<AutoDriveEffectPayload>): List<AutoDriveEffectPayload> {
        return effects.filter { effect ->
            effect.execRequest == null && effect.patchRequest == null &&
                effect.type != "kotlin_exec_request" && effect.type != "kotlin_patch_request"
        }
    }

    private fun describeSequenceMismatch(
        expected: List<AutoDriveSequenceStepPayload>,
        actual: List<AutoDriveSequenceStepPayload>,
    ): String {
        val builder = StringBuilder()
        builder.appendLine("Auto Drive sequence mismatch: expected=${expected.size} steps, actual=${actual.size} steps")
        val max = maxOf(expected.size, actual.size)
        for (index in 0 until max) {
            val expectedStep = expected.getOrNull(index)
            val actualStep = actual.getOrNull(index)
            if (expectedStep != actualStep) {
                builder.appendLine("First mismatch at step $index:")
                builder.appendLine("  expected effects: ${expectedStep?.effects}")
                builder.appendLine("  actual effects  : ${actualStep?.effects}")
                builder.appendLine("  expected snapshot: ${expectedStep?.snapshot}")
                builder.appendLine("  actual snapshot  : ${actualStep?.snapshot}")
                if (expectedStep != null && actualStep != null) {
                    val diffs = describeSnapshotDiff(expectedStep.snapshot, actualStep.snapshot)
                    if (diffs.isNotEmpty()) {
                        builder.appendLine("  snapshot field diffs: ${diffs.joinToString()}")
                    }
                }
                break
            }
        }
        return builder.toString()
    }

    private fun describeSnapshotDiff(
        expected: AutoDriveControllerSnapshotPayload,
        actual: AutoDriveControllerSnapshotPayload,
    ): List<String> {
        val diffs = mutableListOf<String>()
        if (expected.phase != actual.phase) {
            diffs += "phase expected=${expected.phase} actual=${actual.phase}"
        }
        if (expected.continueMode != actual.continueMode) {
            diffs += "continueMode expected=${expected.continueMode} actual=${actual.continueMode}"
        }
        if (expected.countdownId != actual.countdownId) {
            diffs += "countdownId expected=${expected.countdownId} actual=${actual.countdownId}"
        }
        if (expected.countdownDecisionSeq != actual.countdownDecisionSeq) {
            diffs += "countdownDecisionSeq expected=${expected.countdownDecisionSeq} actual=${actual.countdownDecisionSeq}"
        }
        if (expected.secondsRemaining != actual.secondsRemaining) {
            diffs += "secondsRemaining expected=${expected.secondsRemaining} actual=${actual.secondsRemaining}"
        }
        if (expected.transientRestartAttempts != actual.transientRestartAttempts) {
            diffs += "transientRestartAttempts expected=${expected.transientRestartAttempts} actual=${actual.transientRestartAttempts}"
        }
        if (expected.restartToken != actual.restartToken) {
            diffs += "restartToken expected=${expected.restartToken} actual=${actual.restartToken}"
        }
        return diffs
    }

    private fun sampleHistory(): List<JsonElement> = listOf(
        systemMessage("s1"),
        userMessage("u1"),
        assistantMessage("a1"),
        assistantMessage("a2"),
        userMessage("u2"),
        assistantMessage("a3"),
        reasoningItem("thinking"),
        otherItem(),
    )

    private fun userMessage(text: String) = buildJsonObject {
        put("type", "message")
        put("role", "user")
        putJsonArray("content") {
            add(buildJsonObject {
                put("type", "output_text")
                put("text", text)
            })
        }
    }

    private fun assistantMessage(text: String) = buildJsonObject {
        put("type", "message")
        put("role", "assistant")
        putJsonArray("content") {
            add(buildJsonObject {
                put("type", "output_text")
                put("text", text)
            })
        }
    }

    private fun reasoningItem(summary: String) = buildJsonObject {
        put("type", "reasoning")
        put("content", JsonNull)
        put("encrypted_content", JsonNull)
        putJsonArray("summary") {
            add(buildJsonObject {
                put("type", "summary_text")
                put("text", summary)
            })
        }
    }

    private fun systemMessage(text: String) = buildJsonObject {
        put("type", "message")
        put("role", "system")
        putJsonArray("content") {
            add(buildJsonObject {
                put("type", "output_text")
                put("text", text)
            })
        }
    }

    private fun otherItem() = buildJsonObject {
        put("type", "other")
    }

    private fun sampleSnapshotRecords(): List<HistorySnapshotRecordPayload> = listOf(
        HistorySnapshotRecordPayload(
            kind = HistorySnapshotRecordKindPayload.ASSISTANT,
            streamId = "stream-1",
            markdown = "first",
        ),
        HistorySnapshotRecordPayload(
            kind = HistorySnapshotRecordKindPayload.ASSISTANT,
            streamId = "stream-1",
            markdown = "dupe",
        ),
        HistorySnapshotRecordPayload(
            kind = HistorySnapshotRecordKindPayload.USER,
            markdown = "user",
        ),
    )

    private fun sampleTurnFlowRequest(): AutoDriveTurnFlowRequest = AutoDriveTurnFlowRequest(
        initialState = AutoDriveControllerStatePayload(
            phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
            continueMode = AutoContinueModePayload.TEN_SECONDS,
            countdownId = 9,
            countdownDecisionSeq = 4,
        ),
        operations = listOf(
            AutoDriveSequenceOperationPayload.UpdateContinueMode(AutoContinueModePayload.SIXTY_SECONDS),
            AutoDriveSequenceOperationPayload.HandleCountdownTick(countdownId = 10, decisionSeq = 4, secondsLeft = 5),
            AutoDriveSequenceOperationPayload.HandleCountdownTick(countdownId = 10, decisionSeq = 4, secondsLeft = 0),
        ),
        history = sampleHistory(),
        dropLastUserTurns = 1,
        snapshotRecords = sampleSnapshotRecords(),
    )
}
