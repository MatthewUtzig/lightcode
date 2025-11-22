package ai.lightcode.core.engine

import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.add
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import org.junit.jupiter.api.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue
import kotlinx.serialization.SerialName

class CoreEngineHostTest {

    private val json = Json {
        ignoreUnknownKeys = true
        encodeDefaults = true
    }

    @Test
    fun autoDriveSequenceProducesSummaryFromRustEngine() {
        val request = AutoDriveSequenceRequest(
            initialState = AutoDriveControllerStatePayload(
                phase = AutoRunPhasePayload.awaitingCoordinator(promptReady = true),
                continueMode = AutoContinueModePayload.MANUAL,
                countdownId = 1,
                countdownDecisionSeq = 1,
            ),
            operations = listOf(
                AutoDriveSequenceOperationPayload.UpdateContinueMode(AutoContinueModePayload.TEN_SECONDS),
                AutoDriveSequenceOperationPayload.HandleCountdownTick(
                    countdownId = 1,
                    decisionSeq = 1,
                    secondsLeft = 0,
                ),
            ),
        )

        val sessionJson = CoreEngineHost.startSession("{}")
        val sessionId = json.parseToJsonElement(sessionJson).jsonObject["session_id"]!!.jsonPrimitive.long

        val submissionJson = json.encodeToString(AutoDriveSequenceRequest.serializer(), request)
        val submitResult = CoreEngineHost.submitTurn(sessionId.toString(), submissionJson)
        val submitStatus = json.parseToJsonElement(submitResult).jsonObject["status"]!!.jsonPrimitive.content
        assertEquals("ok", submitStatus)

        val pollCursor = json.encodeToString(CursorPayload.serializer(), CursorPayload(cursor = 0))
        val pollResult = CoreEngineHost.pollEvents(sessionId.toString(), pollCursor)
        val poll = json.decodeFromString(HostPoll.serializer(), pollResult)
        assertEquals("ok", poll.status)
        val summaries = poll.events.mapNotNull { it.payload?.jsonObject?.get("message")?.jsonPrimitive?.content }
        assertTrue(summaries.isNotEmpty(), "expected Kotlin host to emit summary messages")

        val direct = CoreEngineApi.autoDriveSequence(request.initialState, request.operations)
        val firstEffect = direct.steps.firstOrNull()?.effects?.firstOrNull()?.type ?: ""
        assertTrue(
            summaries.any { it.contains(firstEffect) },
            "summary should reference real auto-drive effect from Rust engine",
        )
    }

    @Test
    fun chatTurnProducesConversationSummary() {
        val sessionJson = CoreEngineHost.startSession("{}")
        val sessionId = json.parseToJsonElement(sessionJson).jsonObject["session_id"]!!.jsonPrimitive.long

        val history = listOf(sampleUserMessage("Summarize the repo"))
        val turnInput = history + sampleUserMessage("Plan the next Auto Drive goal")
        val submissionElement = buildJsonObject {
            put("type", JsonPrimitive("chat_turn"))
            put("history", buildJsonArray {
                history.forEach { add(it) }
            })
            put("turn_input", buildJsonArray {
                turnInput.forEach { add(it) }
            })
        }
        val submissionJson = json.encodeToString(JsonElement.serializer(), submissionElement)

        val submitResult = CoreEngineHost.submitTurn(sessionId.toString(), submissionJson)
        val submitStatus = json.parseToJsonElement(submitResult).jsonObject["status"]!!.jsonPrimitive.content
        assertEquals("ok", submitStatus)

        val pollCursor = json.encodeToString(CursorPayload.serializer(), CursorPayload(cursor = 0))
        val pollResult = CoreEngineHost.pollEvents(sessionId.toString(), pollCursor)
        val poll = json.decodeFromString(HostPoll.serializer(), pollResult)
        val messages = poll.events.mapNotNull { it.payload?.jsonObject?.get("message")?.jsonPrimitive?.content }
        assertTrue(messages.any { it.contains("Latest user prompt") }, "expected summary to mention latest user prompt")
        assertTrue(messages.any { it.startsWith("Step") }, "expected auto-drive summary events")
    }

    @Test
    fun controlStopEmitsStopAckDecision() {
        val sessionJson = CoreEngineHost.startSession("{}")
        val sessionId = json.parseToJsonElement(sessionJson).jsonObject["session_id"]!!.jsonPrimitive.long

        val submissionElement = buildJsonObject {
            put("type", JsonPrimitive("control"))
            put("command", JsonPrimitive("stop"))
        }
        val submissionJson = json.encodeToString(JsonElement.serializer(), submissionElement)

        val submitResult = CoreEngineHost.submitTurn(sessionId.toString(), submissionJson)
        val submitStatus = json.parseToJsonElement(submitResult).jsonObject["status"]!!.jsonPrimitive.content
        assertEquals("ok", submitStatus)

        val pollCursor = json.encodeToString(CursorPayload.serializer(), CursorPayload(cursor = 0))
        val pollResult = CoreEngineHost.pollEvents(sessionId.toString(), pollCursor)
        val poll = json.decodeFromString(HostPoll.serializer(), pollResult)
        val coordinatorEvent = poll.events.firstOrNull { it.kind == "kotlin_coordinator_event" }
        assertNotNull(coordinatorEvent, "expected stop control to enqueue coordinator event")
        val payload = coordinatorEvent.payload?.jsonObject
        assertNotNull(payload, "expected coordinator event payload")
        val decisions = payload["decisions"]?.jsonArray
        assertNotNull(decisions, "expected decisions array")
        val decisionType = decisions.firstOrNull()?.jsonObject?.get("type")?.jsonPrimitive?.content
        assertEquals("stop_ack", decisionType, "expected stop_ack decision")
    }

    @Serializable
    private data class CursorPayload(val cursor: Long)

    @Serializable
    private data class HostPoll(
        val status: String,
        val events: List<HostEvent>,
        @SerialName("next_cursor") val nextCursor: Long,
    )

    @Serializable
    private data class HostEvent(
        val seq: Long,
        val kind: String,
        val payload: JsonElement? = null,
    )
}

private fun sampleUserMessage(text: String): JsonElement = buildJsonObject {
    put("type", JsonPrimitive("message"))
    put("role", JsonPrimitive("user"))
    put("content", buildJsonArray {
        add(
            buildJsonObject {
                put("type", JsonPrimitive("input_text"))
                put("text", JsonPrimitive(text))
            },
        )
    })
}
