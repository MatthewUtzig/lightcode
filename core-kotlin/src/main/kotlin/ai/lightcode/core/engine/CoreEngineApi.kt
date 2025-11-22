package ai.lightcode.core.engine

import ai.lightcode.core.jni.RustCoreBridge
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.buildJsonObject

/**
 * Convenience Kotlin APIs that wrap the JSON-based CoreEngine bridge.
 */
object CoreEngineApi {
    private val json = Json {
        ignoreUnknownKeys = true
        encodeDefaults = true
    }
    private val debug = System.getenv("CORE_ENGINE_DEBUG") == "1"

    fun echo(payload: JsonElement): EchoResponse {
        val request = EchoRequest(payload = payload)
        val responseJson = RustCoreBridge.execute(json.encodeToString(EchoRequest.serializer(), request))
        logResponse("echo", responseJson)
        return json.decodeFromString(EchoResponse.serializer(), responseJson)
    }

    fun parseIdToken(token: String): ParseIdTokenResponse {
        val request = ParseIdTokenRequest(token = token)
        val responseJson = RustCoreBridge.execute(json.encodeToString(ParseIdTokenRequest.serializer(), request))
        logResponse("parse_id_token", responseJson)
        return json.decodeFromString(ParseIdTokenResponse.serializer(), responseJson)
    }

    fun autoDriveCountdownTick(
        phase: AutoRunPhasePayload,
        countdownId: Long,
        decisionSeq: Long,
        secondsLeft: Int,
    ): AutoDriveCountdownTickResponse {
        require(secondsLeft in 0..255) { "secondsLeft must be between 0 and 255" }
        val request = AutoDriveCountdownTickRequest(
            phase = phase,
            countdownId = countdownId,
            decisionSeq = decisionSeq,
            secondsLeft = secondsLeft,
        )
        val requestJson = json.encodeToString(AutoDriveCountdownTickRequest.serializer(), request)
        val responseJson = RustCoreBridge.execute(requestJson)
        logResponse("auto_drive_countdown_tick", responseJson)
        return json.decodeFromString(AutoDriveCountdownTickResponse.serializer(), responseJson)
    }

    fun autoDriveUpdateContinueMode(
        phase: AutoRunPhasePayload,
        continueMode: AutoContinueModePayload,
        countdownId: Long,
        decisionSeq: Long,
    ): AutoDriveUpdateContinueModeResponse {
        val request = AutoDriveUpdateContinueModeRequest(
            phase = phase,
            continueMode = continueMode,
            countdownId = countdownId,
            decisionSeq = decisionSeq,
        )
        val requestJson = json.encodeToString(AutoDriveUpdateContinueModeRequest.serializer(), request)
        val responseJson = RustCoreBridge.execute(requestJson)
        logResponse("auto_drive_update_continue_mode", responseJson)
        return json.decodeFromString(AutoDriveUpdateContinueModeResponse.serializer(), responseJson)
    }

    fun autoDriveSequence(
        initialState: AutoDriveControllerStatePayload,
        operations: List<AutoDriveSequenceOperationPayload>,
    ): AutoDriveSequenceResponse {
        val request = AutoDriveSequenceRequest(initialState = initialState, operations = operations)
        val requestJson = json.encodeToString(AutoDriveSequenceRequest.serializer(), request)
        val responseJson = RustCoreBridge.execute(requestJson)
        logResponse("auto_drive_sequence", responseJson)
        return json.decodeFromString(AutoDriveSequenceResponse.serializer(), responseJson)
    }

    fun conversationPruneHistory(
        history: List<JsonElement>,
        dropLastUserTurns: Int,
    ): ConversationPruneHistoryResponse {
        require(dropLastUserTurns >= 0) { "dropLastUserTurns must be non-negative" }
        val request = ConversationPruneHistoryRequest(
            history = history,
            dropLastUserTurns = dropLastUserTurns,
        )
        val requestJson = json.encodeToString(ConversationPruneHistoryRequest.serializer(), request)
        val responseJson = RustCoreBridge.execute(requestJson)
        logResponse("conversation_prune_history", responseJson)
        return json.decodeFromString(ConversationPruneHistoryResponse.serializer(), responseJson)
    }

    fun conversationFilterHistory(
        history: List<JsonElement>,
    ): ConversationFilterHistoryResponse {
        val request = ConversationFilterHistoryRequest(history = history)
        val requestJson = json.encodeToString(ConversationFilterHistoryRequest.serializer(), request)
        val responseJson = RustCoreBridge.execute(requestJson)
        logResponse("conversation_filter_history", responseJson)
        return json.decodeFromString(ConversationFilterHistoryResponse.serializer(), responseJson)
    }

    fun conversationCoalesceSnapshot(
        records: List<HistorySnapshotRecordPayload>,
    ): ConversationCoalesceSnapshotResponse {
        val request = ConversationCoalesceSnapshotRequest(records = records)
        val requestJson = json.encodeToString(ConversationCoalesceSnapshotRequest.serializer(), request)
        val responseJson = RustCoreBridge.execute(requestJson)
        logResponse("conversation_coalesce_snapshot", responseJson)
        return json.decodeFromString(ConversationCoalesceSnapshotResponse.serializer(), responseJson)
    }

    fun conversationSnapshotSummary(
        records: List<HistorySnapshotRecordPayload>,
    ): ConversationSnapshotSummaryResponse {
        val request = ConversationSnapshotSummaryRequest(records = records)
        val requestJson = json.encodeToString(ConversationSnapshotSummaryRequest.serializer(), request)
        val responseJson = RustCoreBridge.execute(requestJson)
        logResponse("conversation_snapshot_summary", responseJson)
        return json.decodeFromString(ConversationSnapshotSummaryResponse.serializer(), responseJson)
    }

    fun conversationForkHistory(
        history: List<JsonElement>,
        dropLastUserTurns: Int,
    ): ConversationForkHistoryResponse {
        val request = ConversationForkHistoryRequest(history = history, dropLastUserTurns = dropLastUserTurns)
        val requestJson = json.encodeToString(ConversationForkHistoryRequest.serializer(), request)
        val responseJson = RustCoreBridge.execute(requestJson)
        logResponse("conversation_fork_history", responseJson)
        return json.decodeFromString(ConversationForkHistoryResponse.serializer(), responseJson)
    }

    fun conversationFilterPopularCommands(
        history: List<JsonElement>,
    ): ConversationFilterPopularCommandsResponse {
        val request = ConversationFilterPopularCommandsRequest(history = history)
        val requestJson = json.encodeToString(ConversationFilterPopularCommandsRequest.serializer(), request)
        val responseJson = RustCoreBridge.execute(requestJson)
        logResponse("conversation_filter_popular_commands", responseJson)
        return json.decodeFromString(ConversationFilterPopularCommandsResponse.serializer(), responseJson)
    }

    fun autoCoordinatorPlanningSeed(
        goalText: String,
        includeAgents: Boolean,
    ): AutoCoordinatorPlanningSeedResponse {
        val request = PlannerSeedRequest(goalText = goalText, includeAgents = includeAgents)
        val requestJson = json.encodeToString(PlannerSeedRequest.serializer(), request)
        val responseJson = RustCoreBridge.execute(requestJson)
        logResponse("auto_coordinator_planning_seed", responseJson)
        return json.decodeFromString(AutoCoordinatorPlanningSeedResponse.serializer(), responseJson)
    }

    fun simpleModelTurn(request: SimpleModelTurnRequest): SimpleModelTurnResponse {
        val requestJson = json.encodeToString(SimpleModelTurnRequest.serializer(), request)
        val responseJson = RustCoreBridge.execute(requestJson)
        logResponse("simple_model_turn", responseJson)
        return json.decodeFromString(SimpleModelTurnResponse.serializer(), responseJson)
    }

    fun initialize(config: JsonElement = buildJsonObject { }): Unit =
        RustCoreBridge.initialize(json.encodeToString(JsonElement.serializer(), config))

    fun shutdown() {
        RustCoreBridge.shutdown()
    }

    private fun logResponse(operation: String, responseJson: String) {
        if (debug) {
            println("[CoreEngineApi] $operation <= $responseJson")
        }
    }
}

@Serializable
private data class EchoRequest(
    val type: String = "echo",
    val payload: JsonElement,
)

@Serializable
private data class ParseIdTokenRequest(
    val type: String = "parse_id_token",
    val token: String,
)

@Serializable
data class EchoResponse(
    val status: String,
    val kind: String,
    val payload: JsonElement,
)

@Serializable
data class ParseIdTokenResponse(
    val status: String,
    val kind: String,
    val email: String? = null,
    @SerialName("chatgpt_plan_type")
    val chatgptPlanType: String? = null,
    val message: String? = null,
)

@Serializable
data class AutoDriveCountdownTickRequest(
    val type: String = "auto_drive_countdown_tick",
    val phase: AutoRunPhasePayload,
    @SerialName("countdown_id") val countdownId: Long,
    @SerialName("decision_seq") val decisionSeq: Long,
    @SerialName("seconds_left") val secondsLeft: Int,
)

@Serializable
data class AutoDriveCountdownTickResponse(
    val status: String,
    val kind: String,
    val effects: List<AutoDriveEffectPayload>,
    @SerialName("seconds_left") val secondsLeft: Int,
)

@Serializable
data class AutoDriveEffectPayload(
    val type: String,
    @SerialName("countdown_id") val countdownId: Long? = null,
    @SerialName("decision_seq") val decisionSeq: Long? = null,
    val seconds: Int? = null,
    val message: String? = null,
    val running: Boolean? = null,
    val hint: String? = null,
    val attempt: Int? = null,
    @SerialName("delay_ms") val delayMs: Long? = null,
    val reason: String? = null,
    val token: Long? = null,
    @SerialName("turns_completed") val turnsCompleted: Int? = null,
    @SerialName("duration_ms") val durationMs: Long? = null,
    @SerialName("exec_request") val execRequest: AutoDriveExecRequestPayload? = null,
    @SerialName("patch_request") val patchRequest: AutoDrivePatchRequestPayload? = null,
)

@Serializable
data class AutoDriveExecRequestPayload(
    @SerialName("call_id") val callId: String,
    val command: List<String>,
    val cwd: String? = null,
    val reason: String? = null,
)

@Serializable
data class AutoDrivePatchRequestPayload(
    @SerialName("call_id") val callId: String,
    val changes: Map<String, AutoDrivePatchChangePayload>,
    val reason: String? = null,
    @SerialName("grant_root") val grantRoot: String? = null,
)

@Serializable
data class AutoDrivePatchChangePayload(
    val kind: AutoDrivePatchChangeKind,
    val content: String? = null,
    @SerialName("unified_diff") val unifiedDiff: String? = null,
    @SerialName("move_path") val movePath: String? = null,
)

@Serializable
enum class AutoDrivePatchChangeKind {
    @SerialName("add")
    Add,
    @SerialName("delete")
    Delete,
    @SerialName("update")
    Update,
}

@Serializable
data class AutoDriveUpdateContinueModeRequest(
    val type: String = "auto_drive_update_continue_mode",
    val phase: AutoRunPhasePayload,
    @SerialName("continue_mode") val continueMode: AutoContinueModePayload,
    @SerialName("countdown_id") val countdownId: Long,
    @SerialName("decision_seq") val decisionSeq: Long,
)

@Serializable
data class AutoDriveUpdateContinueModeResponse(
    val status: String,
    val kind: String,
    val effects: List<AutoDriveEffectPayload>,
    @SerialName("seconds_left") val secondsLeft: Int,
)

@Serializable
data class AutoDriveControllerStatePayload(
    val phase: AutoRunPhasePayload,
    @SerialName("continue_mode") val continueMode: AutoContinueModePayload,
    @SerialName("countdown_id") val countdownId: Long = 0,
    @SerialName("countdown_decision_seq") val countdownDecisionSeq: Long = 0,
)

@Serializable
data class AutoDriveSequenceRequest(
    val type: String = "auto_drive_sequence",
    @SerialName("initial_state") val initialState: AutoDriveControllerStatePayload,
    val operations: List<AutoDriveSequenceOperationPayload>,
)

@Serializable
data class AutoDriveSequenceResponse(
    val status: String,
    val kind: String,
    val steps: List<AutoDriveSequenceStepPayload>,
)

@Serializable
data class ConversationPruneHistoryRequest(
    val type: String = "conversation_prune_history",
    val history: List<JsonElement>,
    @SerialName("drop_last_user_turns") val dropLastUserTurns: Int,
)

@Serializable
data class ConversationPruneHistoryResponse(
    val status: String,
    val kind: String,
    val history: List<JsonElement>,
    @SerialName("pruned_user_turns") val prunedUserTurns: Int,
    @SerialName("was_reset") val wasReset: Boolean,
)

@Serializable
data class ConversationFilterHistoryRequest(
    val type: String = "conversation_filter_history",
    val history: List<JsonElement>,
)

@Serializable
data class ConversationFilterHistoryResponse(
    val status: String,
    val kind: String,
    val history: List<JsonElement>,
    @SerialName("removed_count") val removedCount: Int,
)

@Serializable
data class ConversationCoalesceSnapshotRequest(
    val type: String = "conversation_coalesce_snapshot",
    val records: List<HistorySnapshotRecordPayload>,
)

@Serializable
data class ConversationCoalesceSnapshotResponse(
    val status: String,
    val kind: String,
    val records: List<HistorySnapshotRecordPayload>,
    @SerialName("removed_count") val removedCount: Int,
)

@Serializable
data class ConversationSnapshotSummaryRequest(
    val type: String = "conversation_snapshot_summary",
    val records: List<HistorySnapshotRecordPayload>,
)

@Serializable
data class ConversationSnapshotSummaryResponse(
    val status: String,
    val kind: String,
    @SerialName("record_count") val recordCount: Int,
    @SerialName("assistant_messages") val assistantMessages: Int,
    @SerialName("user_messages") val userMessages: Int,
)

@Serializable
data class ConversationForkHistoryRequest(
    val type: String = "conversation_fork_history",
    val history: List<JsonElement>,
    @SerialName("drop_last_user_turns") val dropLastUserTurns: Int,
)

@Serializable
data class ConversationForkHistoryResponse(
    val status: String,
    val kind: String,
    val history: List<JsonElement>,
    @SerialName("dropped_user_turns") val droppedUserTurns: Int,
    @SerialName("became_new") val becameNew: Boolean,
)

@Serializable
data class ConversationFilterPopularCommandsRequest(
    val type: String = "conversation_filter_popular_commands",
    val history: List<JsonElement>,
)

@Serializable
data class ConversationFilterPopularCommandsResponse(
    val status: String,
    val kind: String,
    val history: List<JsonElement>,
)

@Serializable
data class PlannerSeedRequest(
    val type: String = "auto_coordinator_planning_seed",
    @SerialName("goal_text") val goalText: String,
    @SerialName("include_agents") val includeAgents: Boolean,
)

@Serializable
data class AutoCoordinatorPlanningSeedResponse(
    val status: String,
    val kind: String,
    @SerialName("response_json") val responseJson: String? = null,
    @SerialName("cli_prompt") val cliPrompt: String? = null,
    @SerialName("goal_message") val goalMessage: String? = null,
    @SerialName("status_title") val statusTitle: String? = null,
    @SerialName("status_sent_to_user") val statusSentToUser: String? = null,
    @SerialName("agents_timing") val agentsTiming: AutoTurnAgentsTimingPayload? = null,
)

@Serializable
enum class AutoTurnAgentsTimingPayload {
    @SerialName("Parallel")
    PARALLEL,
}

@Serializable
data class HistorySnapshotRecordPayload(
    val kind: HistorySnapshotRecordKindPayload,
    @SerialName("stream_id") val streamId: String? = null,
    val markdown: String? = null,
)

@Serializable
enum class HistorySnapshotRecordKindPayload {
    @SerialName("assistant")
    ASSISTANT,
    @SerialName("user")
    USER,
    @SerialName("system")
    SYSTEM,
    @SerialName("other")
    OTHER,
}

@Serializable
data class AutoDriveSequenceStepPayload(
    val effects: List<AutoDriveEffectPayload>,
    val snapshot: AutoDriveControllerSnapshotPayload,
)

@Serializable
data class AutoDriveControllerSnapshotPayload(
    val phase: AutoRunPhasePayload,
    @SerialName("continue_mode") val continueMode: AutoContinueModePayload,
    @SerialName("countdown_id") val countdownId: Long,
    @SerialName("countdown_decision_seq") val countdownDecisionSeq: Long,
    @SerialName("seconds_remaining") val secondsRemaining: Int,
    @SerialName("transient_restart_attempts") val transientRestartAttempts: Int,
    @SerialName("restart_token") val restartToken: Long,
)

@Serializable
sealed interface AutoDriveSequenceOperationPayload {
    @Serializable
    @SerialName("update_continue_mode")
    data class UpdateContinueMode(val mode: AutoContinueModePayload) : AutoDriveSequenceOperationPayload

    @Serializable
    @SerialName("handle_countdown_tick")
    data class HandleCountdownTick(
        @SerialName("countdown_id") val countdownId: Long,
        @SerialName("decision_seq") val decisionSeq: Long,
        @SerialName("seconds_left") val secondsLeft: Int,
    ) : AutoDriveSequenceOperationPayload

    @Serializable
    @SerialName("pause_for_transient_failure")
    data class PauseForTransientFailure(val reason: String) : AutoDriveSequenceOperationPayload

    @Serializable
    @SerialName("stop_run")
    data class StopRun(val message: String? = null) : AutoDriveSequenceOperationPayload

    @Serializable
    @SerialName("launch_result")
    data class LaunchResult(
        val result: LaunchOutcome,
        val goal: String,
        val error: String? = null,
    ) : AutoDriveSequenceOperationPayload

    @Serializable
    enum class LaunchOutcome {
        @SerialName("succeeded")
        SUCCEEDED,
        @SerialName("failed")
        FAILED,
    }
}

@Serializable
data class AutoRunPhasePayload(
    val name: AutoRunPhaseName,
    @SerialName("resume_after_submit") val resumeAfterSubmit: Boolean? = null,
    @SerialName("bypass_next_submit") val bypassNextSubmit: Boolean? = null,
    @SerialName("prompt_ready") val promptReady: Boolean? = null,
    @SerialName("coordinator_waiting") val coordinatorWaiting: Boolean? = null,
    @SerialName("diagnostics_pending") val diagnosticsPending: Boolean? = null,
    @SerialName("backoff_ms") val backoffMs: Long? = null,
) {
    companion object {
        fun idle() = AutoRunPhasePayload(name = AutoRunPhaseName.IDLE)
        fun active() = AutoRunPhasePayload(name = AutoRunPhaseName.ACTIVE)
        fun awaitingGoalEntry() = AutoRunPhasePayload(name = AutoRunPhaseName.AWAITING_GOAL_ENTRY)
        fun launching() = AutoRunPhasePayload(name = AutoRunPhaseName.LAUNCHING)
        fun awaitingCoordinator(promptReady: Boolean) = AutoRunPhasePayload(
            name = AutoRunPhaseName.AWAITING_COORDINATOR,
            promptReady = promptReady,
        )
        fun pausedManual(resumeAfterSubmit: Boolean, bypassNextSubmit: Boolean) = AutoRunPhasePayload(
            name = AutoRunPhaseName.PAUSED_MANUAL,
            resumeAfterSubmit = resumeAfterSubmit,
            bypassNextSubmit = bypassNextSubmit,
        )
        fun awaitingDiagnostics(coordinatorWaiting: Boolean) = AutoRunPhasePayload(
            name = AutoRunPhaseName.AWAITING_DIAGNOSTICS,
            coordinatorWaiting = coordinatorWaiting,
        )
        fun awaitingReview(diagnosticsPending: Boolean) = AutoRunPhasePayload(
            name = AutoRunPhaseName.AWAITING_REVIEW,
            diagnosticsPending = diagnosticsPending,
        )
        fun transientRecovery(backoffMs: Long) = AutoRunPhasePayload(
            name = AutoRunPhaseName.TRANSIENT_RECOVERY,
            backoffMs = backoffMs,
        )
    }
}

@Serializable
enum class AutoRunPhaseName {
    @SerialName("idle")
    IDLE,
    @SerialName("awaiting_goal_entry")
    AWAITING_GOAL_ENTRY,
    @SerialName("launching")
    LAUNCHING,
    @SerialName("active")
    ACTIVE,
    @SerialName("paused_manual")
    PAUSED_MANUAL,
    @SerialName("awaiting_coordinator")
    AWAITING_COORDINATOR,
    @SerialName("awaiting_diagnostics")
    AWAITING_DIAGNOSTICS,
    @SerialName("awaiting_review")
    AWAITING_REVIEW,
    @SerialName("transient_recovery")
    TRANSIENT_RECOVERY,
}

@Serializable
enum class AutoContinueModePayload {
    @SerialName("immediate")
    IMMEDIATE,
    @SerialName("ten_seconds")
    TEN_SECONDS,
    @SerialName("sixty_seconds")
    SIXTY_SECONDS,
    @SerialName("manual")
    MANUAL,
}

@Serializable
data class SimpleModelTurnRequest(
    val type: String = "simple_model_turn",
    val history: List<JsonElement> = emptyList(),
    @SerialName("latest_user_prompt") val latestUserPrompt: String? = null,
)

@Serializable
data class SimpleModelTurnResponse(
    val status: String,
    val kind: String,
    val thinking: List<String> = emptyList(),
    val answer: String = "",
    val message: String? = null,
    @SerialName("token_usage") val tokenUsage: TokenUsagePayload? = null,
)

@Serializable
data class TokenUsagePayload(
    @SerialName("input_tokens") val inputTokens: Int = 0,
    @SerialName("cached_input_tokens") val cachedInputTokens: Int = 0,
    @SerialName("output_tokens") val outputTokens: Int = 0,
    @SerialName("reasoning_output_tokens") val reasoningOutputTokens: Int = 0,
    @SerialName("total_tokens") val totalTokens: Int = 0,
)
