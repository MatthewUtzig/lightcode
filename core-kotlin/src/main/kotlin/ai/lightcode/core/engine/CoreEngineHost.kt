package ai.lightcode.core.engine

import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong
import ai.lightcode.core.engine.coordinator.KotlinCoordinatorDecision
import ai.lightcode.core.engine.coordinator.KotlinCoordinatorInput
import ai.lightcode.core.engine.coordinator.KotlinCoordinatorResult
import ai.lightcode.core.engine.coordinator.KotlinTokenMetrics
import ai.lightcode.core.engine.coordinator.KotlinTokenUsage
import ai.lightcode.core.engine.coordinator.SimpleKotlinCoordinator
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/**
 * Minimal session-oriented host facade so the Rust CLI can drive the Kotlin
 * engine end-to-end via JNI.
 */
object CoreEngineHost {

    internal val json = Json {
        ignoreUnknownKeys = true
        encodeDefaults = true
    }

    private val sessionCounter = AtomicLong(1)
    private val sessions = ConcurrentHashMap<Long, SessionState>()

    @JvmStatic
    fun startSession(configJson: String): String {
        val sessionId = sessionCounter.getAndIncrement()
        sessions[sessionId] = SessionState()
        return json.encodeToString(
            StartSessionResponse(
                status = "ok",
                sessionId = sessionId,
            ),
        )
    }

    @JvmStatic
    fun runAutoDriveSequenceRaw(requestJson: String): String {
        val request = runCatching {
            json.decodeFromString(SubmissionEnvelope.AutoDriveSequence.serializer(), requestJson)
        }.getOrElse {
            return json.encodeToString(error("invalid_submission"))
        }
        val response = runCatching {
            CoreEngineApi.autoDriveSequence(
                initialState = request.initialState,
                operations = request.operations,
            )
        }.getOrElse {
            return json.encodeToString(error("engine_failure"))
        }
        return json.encodeToString(AutoDriveSequenceResponse.serializer(), response)
    }

    @JvmStatic
    fun submitTurn(sessionIdRaw: String, submissionJson: String): String {
        val sessionId = sessionIdRaw.toLongOrNull()
            ?: return json.encodeToString(error("invalid_session_id"))
        val state = sessions[sessionId]
            ?: return json.encodeToString(error("session_not_found"))

        val envelope = runCatching {
            json.decodeFromString(SubmissionEnvelope.serializer(), submissionJson)
        }.getOrElse {
            return json.encodeToString(error("invalid_submission"))
        }

        val status = when (envelope) {
            is SubmissionEnvelope.AutoDriveSequence -> state.handleAutoDriveSequence(envelope)
            is SubmissionEnvelope.ChatTurn -> state.handleChatTurn(envelope)
            is SubmissionEnvelope.Control -> state.handleControl(envelope.command)
        }

        return json.encodeToString(status)
    }

    @JvmStatic
    fun pollEvents(sessionIdRaw: String, cursorJson: String): String {
        val sessionId = sessionIdRaw.toLongOrNull()
            ?: return json.encodeToString(error("invalid_session_id"))
        val state = sessions[sessionId]
            ?: return json.encodeToString(error("session_not_found"))

        val cursor = runCatching {
            if (cursorJson.isBlank()) 0L else json.decodeFromString(CursorPayload.serializer(), cursorJson).cursor
        }.getOrElse { 0L }

        val events = state.events.filter { it.seq >= cursor }
        val nextCursor = events.maxOfOrNull { it.seq + 1 } ?: cursor

        return json.encodeToString(
            PollResponse(
                status = "ok",
                events = events,
                nextCursor = nextCursor,
                hasMore = false,
            ),
        )
    }

    @JvmStatic
    fun closeSession(sessionIdRaw: String): String {
        val sessionId = sessionIdRaw.toLongOrNull()
            ?: return json.encodeToString(error("invalid_session_id"))
        sessions.remove(sessionId)
        return json.encodeToString(SimpleStatus(status = "ok"))
    }

    private fun error(code: String) = SimpleStatus(status = "error", reason = code)

    internal fun serializeEffect(effect: AutoDriveEffectPayload): JsonElement =
        json.encodeToJsonElement(AutoDriveEffectPayload.serializer(), effect)
}

private fun extractLatestUserText(items: List<JsonElement>): String? {
    return items.asReversed().firstNotNullOfOrNull { element ->
        val obj = element.jsonObject
        val type = obj["type"]?.jsonPrimitive?.contentOrNull
        val role = obj["role"]?.jsonPrimitive?.contentOrNull
        if (type != "message" || role != "user") {
            return@firstNotNullOfOrNull null
        }
        val contentArray = obj["content"]?.jsonArray ?: return@firstNotNullOfOrNull null
        val text = contentArray.asReversed().firstNotNullOfOrNull { contentItem ->
            val contentObj = contentItem.jsonObject
            val contentType = contentObj["type"]?.jsonPrimitive?.contentOrNull
            if (contentType == "input_text") {
                contentObj["text"]?.jsonPrimitive?.contentOrNull
            } else {
                null
            }
        }
        text?.trim().takeUnless { it.isNullOrEmpty() }
    }
}

private fun truncateForSummary(text: String, limit: Int = 200): String {
    val trimmed = text.trim()
    return if (trimmed.length <= limit) trimmed else trimmed.substring(0, limit) + "…"
}

private fun buildThinkingLine(prompt: String?): String {
    val snippet = prompt?.let { truncateForSummary(it, limit = 160) }
    return if (snippet != null) {
        "Thinking through \"${snippet}\" with the Kotlin coordinator…"
    } else {
        "Thinking through the latest Kotlin request…"
    }
}

private fun buildAnswerLine(prompt: String?): String {
    val snippet = prompt?.let { truncateForSummary(it, limit = 160) }
    return if (snippet != null) {
        "Answering \"${snippet}\" directly from the Kotlin engine so exec/patch can follow."
    } else {
        "Answering from the Kotlin engine—share a concrete request for a deeper plan."
    }
}

private object KotlinCoordinatorBridge {
    private val coordinator = SimpleKotlinCoordinator()

    fun runToolFreeTurn(
        history: List<JsonElement>,
        latestUserPrompt: String?,
    ): KotlinCoordinatorResult? {
        if (history.isEmpty()) {
            return null
        }
        return runCatching {
            coordinator.runTurn(
                KotlinCoordinatorInput(
                    history = history,
                    latestUserPrompt = latestUserPrompt,
                ),
            )
        }.getOrElse { throwable ->
            KotlinCoordinatorResult(
                decisions = emptyList(),
                fallbackReason = throwable.message,
            )
        }
    }
}

private fun buildDefaultChatSequence(goalText: String?): Pair<AutoDriveControllerStatePayload, List<AutoDriveSequenceOperationPayload>> {
    val goal = goalText?.takeIf { it.isNotBlank() } ?: "Describe your goal for this turn."
    val state = AutoDriveControllerStatePayload(
        phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
        continueMode = AutoContinueModePayload.MANUAL,
        countdownId = 1,
        countdownDecisionSeq = 1,
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
            goal = truncateForSummary(goal, limit = 512),
            error = null,
        ),
        AutoDriveSequenceOperationPayload.StopRun(message = "Kotlin run complete"),
    )
    return state to operations
}

@Serializable
private sealed interface SubmissionEnvelope {

    @Serializable
    @SerialName("auto_drive_sequence")
    data class AutoDriveSequence(
        @SerialName("initial_state") val initialState: AutoDriveControllerStatePayload,
        val operations: List<AutoDriveSequenceOperationPayload>,
    ) : SubmissionEnvelope

    @Serializable
    @SerialName("chat_turn")
    data class ChatTurn(
        val history: List<JsonElement> = emptyList(),
        @SerialName("turn_input") val turnInput: List<JsonElement> = emptyList(),
    ) : SubmissionEnvelope

    @Serializable
    @SerialName("control")
    data class Control(
        val command: ControlCommand,
    ) : SubmissionEnvelope
}

@Serializable
private enum class ControlCommand {
    @SerialName("stop")
    STOP,
}

private class SessionState {
    val events: MutableList<EngineEvent> = mutableListOf()
    val nextSeq = AtomicLong(0)

    fun enqueueMessage(text: String) {
        val payload = buildJsonObject {
            put("message", text)
        }
        enqueueEvent(EngineEventKind.AGENT_MESSAGE, payload)
    }

    fun enqueueAutoDriveSummary(response: AutoDriveSequenceResponse) {
        response.steps.forEachIndexed { index, step ->
            val effects = if (step.effects.isEmpty()) {
                "no effects"
            } else {
                step.effects.joinToString { it.type }
            }
            val phaseName = step.snapshot.phase.name.name.lowercase()
            val message = "Step ${index + 1} (${phaseName}): $effects"
            enqueueMessage(message)
        }
        if (response.steps.isEmpty()) {
            enqueueMessage("AutoDrive produced no steps")
        }
    }

    fun handleControl(command: ControlCommand): SimpleStatus {
        return when (command) {
            ControlCommand.STOP -> {
                enqueueCoordinatorDecisions(
                    KotlinCoordinatorResult(
                        decisions = listOf(KotlinCoordinatorDecision.StopAcknowledged),
                    ),
                )
                SimpleStatus(status = "ok")
            }
        }
    }

    fun enqueueCoordinatorDecisions(result: KotlinCoordinatorResult) {
        if (result.decisions.isNotEmpty() || result.tokenMetrics != null) {
            val payload = buildJsonObject {
                put(
                    "decisions",
                    buildJsonArray {
                        result.decisions.forEach { decision ->
                            add(serializeCoordinatorDecision(decision))
                        }
                    },
                )
                result.tokenMetrics?.let {
                    put("token_metrics", serializeTokenMetrics(it))
                }
            }
            enqueueEvent(EngineEventKind.KOTLIN_COORDINATOR_EVENT, payload)
        }

        result.decisions.forEach { decision ->
            when (decision) {
                is KotlinCoordinatorDecision.Thinking -> enqueueMessage(decision.text)
                is KotlinCoordinatorDecision.FinalAnswer -> enqueueMessage(decision.text)
                is KotlinCoordinatorDecision.RequestExecCommand -> {
                    val preview = truncateForSummary(decision.preview, limit = 120)
                    val rationale = decision.rationale?.let { ": ${truncateForSummary(it, limit = 120)}" } ?: ""
                    enqueueMessage("Kotlin coordinator pending exec${rationale}: ${preview}")
                }
                is KotlinCoordinatorDecision.RequestApplyPatch -> {
                    val preview = truncateForSummary(decision.preview, limit = 120)
                    val rationale = decision.rationale?.let { ": ${truncateForSummary(it, limit = 120)}" } ?: ""
                    enqueueMessage("Kotlin coordinator pending patch${rationale}: ${preview}")
                }
                is KotlinCoordinatorDecision.StopAcknowledged -> {
                    enqueueMessage("Kotlin coordinator stopped per user request")
                }
            }
        }
    }

    private fun serializeCoordinatorDecision(decision: KotlinCoordinatorDecision) = buildJsonObject {
        when (decision) {
            is KotlinCoordinatorDecision.Thinking -> {
                put("type", "thinking")
                put("text", decision.text)
                decision.summaryIndex?.let { put("summary_index", it) }
            }
            is KotlinCoordinatorDecision.FinalAnswer -> {
                put("type", "final_answer")
                put("text", decision.text)
            }
            is KotlinCoordinatorDecision.RequestExecCommand -> {
                put("type", "request_exec_command")
                put("command", decision.command)
                put("preview", decision.preview)
                decision.rationale?.let { put("rationale", it) }
            }
            is KotlinCoordinatorDecision.RequestApplyPatch -> {
                put("type", "request_apply_patch")
                put("patch", decision.patch)
                put("preview", decision.preview)
                decision.rationale?.let { put("rationale", it) }
            }
            is KotlinCoordinatorDecision.StopAcknowledged -> {
                put("type", "stop_ack")
            }
        }
    }

    private fun serializeTokenMetrics(metrics: KotlinTokenMetrics) = buildJsonObject {
        put("total_usage", serializeTokenUsage(metrics.totalUsage))
        put("last_turn_usage", serializeTokenUsage(metrics.lastTurnUsage))
        put("turn_count", metrics.turnCount)
        put("duplicate_items", metrics.duplicateItems)
        put("replay_updates", metrics.replayUpdates)
    }

    private fun serializeTokenUsage(usage: KotlinTokenUsage) = buildJsonObject {
        put("input_tokens", usage.inputTokens)
        put("cached_input_tokens", usage.cachedInputTokens)
        put("output_tokens", usage.outputTokens)
        put("reasoning_output_tokens", usage.reasoningOutputTokens)
        put("total_tokens", usage.totalTokens)
    }

    fun handleAutoDriveSequence(request: SubmissionEnvelope.AutoDriveSequence): SimpleStatus {
        return runAutoDriveSequence(
            request.initialState,
            request.operations,
            goalText = null,
            propagateError = true,
        )
    }

    fun handleChatTurn(request: SubmissionEnvelope.ChatTurn): SimpleStatus {
        val combinedHistory = request.history + request.turnInput
        val filteredHistory = runCatching {
            CoreEngineApi.conversationFilterHistory(combinedHistory)
        }.getOrNull()

        val removedCount = filteredHistory?.removedCount ?: 0
        val filteredItems = filteredHistory?.history ?: combinedHistory
        val latestUserPrompt = extractLatestUserText(request.turnInput)
            ?: extractLatestUserText(filteredItems)

        val coordinatorResult = KotlinCoordinatorBridge.runToolFreeTurn(filteredItems, latestUserPrompt)
        if (coordinatorResult?.isUsable == true) {
            enqueueCoordinatorDecisions(coordinatorResult)
        } else {
            coordinatorResult?.fallbackReason?.let {
                enqueueMessage("Kotlin coordinator fallback: ${truncateForSummary(it, limit = 120)}")
            }
            enqueueMessage(buildThinkingLine(latestUserPrompt))
            enqueueMessage(buildAnswerLine(latestUserPrompt))
        }

        val summaryText = buildString {
            append("Kotlin engine processed ${filteredItems.size} items")
            if (removedCount > 0) {
                append(" (filtered $removedCount)")
            }
            latestUserPrompt?.let {
                append(". Latest user prompt: \"")
                append(truncateForSummary(it))
                append("\"")
            }
        }
        enqueueMessage(summaryText)

        val (state, operations) = buildDefaultChatSequence(latestUserPrompt)
        return runAutoDriveSequence(
            state,
            operations,
            goalText = latestUserPrompt,
            propagateError = false,
        )
    }

    private fun runAutoDriveSequence(
        initialState: AutoDriveControllerStatePayload,
        operations: List<AutoDriveSequenceOperationPayload>,
        goalText: String?,
        propagateError: Boolean,
    ): SimpleStatus {
        val response = runCatching {
            CoreEngineApi.autoDriveSequence(initialState = initialState, operations = operations)
        }.getOrElse {
            if (propagateError) {
                return SimpleStatus(status = "error", reason = "engine_failure")
            }
            enqueueMessage("AutoDrive sequence failed: ${it.message ?: "unknown error"}")
            return SimpleStatus(status = "ok")
        }

        enqueueAutoDriveSummary(response)
        enqueueAutoDriveInstrumentation(response, goalText)
        return SimpleStatus(status = "ok")
    }

    private fun enqueueAutoDriveInstrumentation(
        response: AutoDriveSequenceResponse,
        goalText: String?,
    ) {
        if (response.steps.isEmpty()) {
            return
        }
        val totalSteps = response.steps.size
        response.steps.forEachIndexed { idx, step ->
            enqueueAutoDriveStepEvent(idx, totalSteps, step)
            enqueueAutoDriveStatusEvent(idx, totalSteps, step, goalText)
        }
    }

    private fun enqueueAutoDriveStepEvent(
        stepIndex: Int,
        totalSteps: Int,
        step: AutoDriveSequenceStepPayload,
    ) {
        val effects = step.effects.map { it.type }
        val payload = buildJsonObject {
            put("step_index", stepIndex)
            put("total_steps", totalSteps)
            put("phase", step.snapshot.phase.name.name.lowercase())
            put("continue_mode", step.snapshot.continueMode.name.lowercase())
            put("seconds_remaining", step.snapshot.secondsRemaining)
            put(
                "effects",
                buildJsonArray {
                    step.effects.forEach { effect ->
                        add(CoreEngineHost.serializeEffect(effect))
                    }
                },
            )
            put("summary", "Step ${stepIndex + 1} of $totalSteps: ${effects.joinToString()}")
        }
        enqueueEvent(EngineEventKind.AUTO_DRIVE_STEP, payload)
    }

    private fun enqueueAutoDriveStatusEvent(
        stepIndex: Int,
        totalSteps: Int,
        step: AutoDriveSequenceStepPayload,
        goalText: String?,
    ) {
        val payload = buildJsonObject {
            put("status", step.snapshot.phase.name.name.lowercase())
            put("step_index", stepIndex)
            put("total_steps", totalSteps)
            put("summary", "Step ${stepIndex + 1} of $totalSteps in ${step.snapshot.phase.name.name.lowercase()}")
            goalText?.let { put("goal", it) }
        }
        enqueueEvent(EngineEventKind.AUTO_DRIVE_STATUS, payload)
    }

private fun enqueueEvent(kind: EngineEventKind, payload: JsonElement) {
        val seq = nextSeq.getAndIncrement()
        events.add(
            EngineEvent(
                seq = seq,
                kind = kind,
                payload = payload,
            ),
        )
    }
}

@Serializable
private data class SimpleStatus(
    val status: String,
    val reason: String? = null,
)

@Serializable
private data class StartSessionResponse(
    val status: String,
    @SerialName("session_id") val sessionId: Long,
)

@Serializable
private data class CursorPayload(
    val cursor: Long = 0,
)

@Serializable
private data class PollResponse(
    val status: String,
    val events: List<EngineEvent>,
    @SerialName("next_cursor") val nextCursor: Long,
    @SerialName("has_more") val hasMore: Boolean,
)

@Serializable
private data class EngineEvent(
    val seq: Long,
    val kind: EngineEventKind,
    val payload: JsonElement,
)

@Serializable
private enum class EngineEventKind {
    @SerialName("agent_message")
    AGENT_MESSAGE,
    @SerialName("auto_drive_step")
    AUTO_DRIVE_STEP,
    @SerialName("auto_drive_status")
    AUTO_DRIVE_STATUS,
    @SerialName("kotlin_coordinator_event")
    KOTLIN_COORDINATOR_EVENT,
}
