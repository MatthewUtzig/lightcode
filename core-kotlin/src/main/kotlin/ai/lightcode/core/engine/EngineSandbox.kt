package ai.lightcode.core.engine

import kotlin.system.exitProcess
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.encodeToJsonElement

private val sandboxJson = Json {
    ignoreUnknownKeys = true
    prettyPrint = true
}

fun main(args: Array<String>) {
    val input = readInput(args)
    val request = try {
        sandboxJson.decodeFromString(SandboxRequest.serializer(), input)
    } catch (err: Exception) {
        emitError("Invalid request: ${err.message}")
        exitProcess(1)
    }

    val response = try {
        CoreEngineApi.initialize()
        handleRequest(request)
    } catch (err: Exception) {
        emitError("Sandbox failure: ${err.message}")
        exitProcess(1)
    } finally {
        runCatching { CoreEngineApi.shutdown() }
    }

    println(sandboxJson.encodeToString(SandboxSuccessResponse.serializer(), response))
}

private fun handleRequest(request: SandboxRequest): SandboxSuccessResponse =
    when (request.operation) {
        SandboxOperation.AUTO_DRIVE_SEQUENCE -> {
            val initial = requireNotNull(request.initialState) {
                "initial_state is required"
            }
            val operations = requireNotNull(request.operations) {
                "operations is required"
            }
            val local = CoreEngineFacade.AutoDrive.runSequenceLocally(initial, operations)
            val rust = CoreEngineFacade.AutoDrive.runSequenceViaRust(initial, operations)
            SandboxSuccessResponse(
                status = "ok",
                kind = "auto_drive_sequence",
                local = sandboxJson.encodeToJsonElement(AutoDriveSequenceResponse.serializer(), local),
                rust = sandboxJson.encodeToJsonElement(AutoDriveSequenceResponse.serializer(), rust),
            )
        }
        SandboxOperation.CONVERSATION_PRUNE_HISTORY -> {
            val history = request.history ?: error("history is required")
            val drop = request.dropLastUserTurns ?: 0
            val local = CoreEngineFacade.Conversation.pruneHistoryLocally(history, drop)
            val rust = CoreEngineFacade.Conversation.pruneHistoryViaRust(history, drop)
            SandboxSuccessResponse(
                status = "ok",
                kind = "conversation_prune_history",
                local = sandboxJson.encodeToJsonElement(ConversationPruneHistoryResponse.serializer(), local),
                rust = sandboxJson.encodeToJsonElement(ConversationPruneHistoryResponse.serializer(), rust),
            )
        }
        SandboxOperation.CONVERSATION_FILTER_HISTORY -> {
            val history = request.history ?: error("history is required")
            val local = CoreEngineFacade.Conversation.filterHistoryLocally(history)
            val rust = CoreEngineFacade.Conversation.filterHistoryViaRust(history)
            SandboxSuccessResponse(
                status = "ok",
                kind = "conversation_filter_history",
                local = sandboxJson.encodeToJsonElement(ConversationFilterHistoryResponse.serializer(), local),
                rust = sandboxJson.encodeToJsonElement(ConversationFilterHistoryResponse.serializer(), rust),
            )
        }
        SandboxOperation.CONVERSATION_COALESCE_SNAPSHOT -> {
            val records = request.snapshotRecords ?: error("snapshot_records is required")
            val local = CoreEngineFacade.Conversation.coalesceSnapshotLocally(records)
            val rust = CoreEngineFacade.Conversation.coalesceSnapshotViaRust(records)
            SandboxSuccessResponse(
                status = "ok",
                kind = "conversation_coalesce_snapshot",
                local = sandboxJson.encodeToJsonElement(ConversationCoalesceSnapshotResponse.serializer(), local),
                rust = sandboxJson.encodeToJsonElement(ConversationCoalesceSnapshotResponse.serializer(), rust),
            )
        }
        SandboxOperation.AUTO_DRIVE_TURN_FLOW -> {
            val turnFlow = request.turnFlow ?: error("turn_flow is required")
            val result = TurnFlowOrchestrator.run(turnFlow)
            SandboxSuccessResponse(
                status = "ok",
                kind = "auto_drive_turn_flow",
                payload = sandboxJson.encodeToJsonElement(AutoDriveTurnFlowResultPayload.serializer(), result),
            )
        }
        SandboxOperation.CONVERSATION_SNAPSHOT_SUMMARY -> {
            val records = request.snapshotRecords ?: error("snapshot_records is required")
            val local = CoreEngineFacade.Conversation.snapshotSummaryLocally(records)
            val rust = CoreEngineFacade.Conversation.snapshotSummaryViaRust(records)
            SandboxSuccessResponse(
                status = "ok",
                kind = "conversation_snapshot_summary",
                local = sandboxJson.encodeToJsonElement(ConversationSnapshotSummaryResponse.serializer(), local),
                rust = sandboxJson.encodeToJsonElement(ConversationSnapshotSummaryResponse.serializer(), rust),
            )
        }
        SandboxOperation.CONVERSATION_FORK_HISTORY -> {
            val history = request.history ?: error("history is required")
            val drop = request.dropLastUserTurns ?: 0
            val local = CoreEngineFacade.Conversation.forkHistoryLocally(history, drop)
            val rust = CoreEngineFacade.Conversation.forkHistoryViaRust(history, drop)
            SandboxSuccessResponse(
                status = "ok",
                kind = "conversation_fork_history",
                local = sandboxJson.encodeToJsonElement(ConversationForkHistoryResponse.serializer(), local),
                rust = sandboxJson.encodeToJsonElement(ConversationForkHistoryResponse.serializer(), rust),
            )
        }
        SandboxOperation.AUTO_COORDINATOR_PLANNING_SEED -> {
            val goal = request.goal ?: error("goal is required")
            val includeAgents = request.includeAgents ?: false
            val local = CoreEngineFacade.Conversation.planningSeedLocally(goal, includeAgents)
            val rust = CoreEngineFacade.Conversation.planningSeedViaRust(goal, includeAgents)
            SandboxSuccessResponse(
                status = "ok",
                kind = "auto_coordinator_planning_seed",
                local = sandboxJson.encodeToJsonElement(AutoCoordinatorPlanningSeedResponse.serializer(), local),
                rust = sandboxJson.encodeToJsonElement(AutoCoordinatorPlanningSeedResponse.serializer(), rust),
            )
        }
    }

private fun readInput(args: Array<String>): String {
    if (args.isNotEmpty()) {
        return args.joinToString(" ").trim()
    }
    val buffered = generateSequence(::readLine)
        .joinToString("\n")
        .trim()
    if (buffered.isEmpty()) {
        error("Expected JSON request via stdin or as the first argument")
    }
    return buffered
}

private fun emitError(message: String) {
    val payload = sandboxJson.encodeToString(
        SandboxErrorResponse(status = "error", message = message),
    )
    System.err.println(payload)
}

@Serializable
private data class SandboxRequest(
    val operation: SandboxOperation,
    @SerialName("initial_state") val initialState: AutoDriveControllerStatePayload? = null,
    val operations: List<AutoDriveSequenceOperationPayload>? = null,
    val history: List<JsonElement>? = null,
    @SerialName("drop_last_user_turns") val dropLastUserTurns: Int? = null,
    @SerialName("snapshot_records") val snapshotRecords: List<HistorySnapshotRecordPayload>? = null,
    @SerialName("turn_flow") val turnFlow: AutoDriveTurnFlowRequest? = null,
    val goal: String? = null,
    @SerialName("include_agents") val includeAgents: Boolean? = null,
)

@Serializable
private enum class SandboxOperation {
    @SerialName("auto_drive_sequence")
    AUTO_DRIVE_SEQUENCE,

    @SerialName("conversation_prune_history")
    CONVERSATION_PRUNE_HISTORY,

    @SerialName("conversation_filter_history")
    CONVERSATION_FILTER_HISTORY,

    @SerialName("conversation_coalesce_snapshot")
    CONVERSATION_COALESCE_SNAPSHOT,

    @SerialName("auto_drive_turn_flow")
    AUTO_DRIVE_TURN_FLOW,

    @SerialName("conversation_snapshot_summary")
    CONVERSATION_SNAPSHOT_SUMMARY,

    @SerialName("conversation_fork_history")
    CONVERSATION_FORK_HISTORY,

    @SerialName("auto_coordinator_planning_seed")
    AUTO_COORDINATOR_PLANNING_SEED,
}

@Serializable
private data class SandboxSuccessResponse(
    val status: String,
    val kind: String,
    val local: JsonElement? = null,
    val rust: JsonElement? = null,
    val payload: JsonElement? = null,
)

@Serializable
private data class SandboxErrorResponse(
    val status: String,
    val message: String,
)
