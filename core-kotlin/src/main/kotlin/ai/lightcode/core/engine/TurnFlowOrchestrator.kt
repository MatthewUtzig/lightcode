package ai.lightcode.core.engine

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement

object TurnFlowOrchestrator {

    fun run(request: AutoDriveTurnFlowRequest): AutoDriveTurnFlowResultPayload {
        val autoSequenceLocal = CoreEngineFacade.AutoDrive.runSequenceLocally(
            request.initialState,
            request.operations,
        )
        val autoSequenceRust = CoreEngineFacade.AutoDrive.runSequenceViaRust(
            request.initialState,
            request.operations,
        )

        val pruneLocal = CoreEngineFacade.Conversation.pruneHistoryLocally(
            request.history,
            request.dropLastUserTurns,
        )
        val pruneRust = CoreEngineFacade.Conversation.pruneHistoryViaRust(
            request.history,
            request.dropLastUserTurns,
        )

        val filterLocal = CoreEngineFacade.Conversation.filterHistoryLocally(pruneLocal.history)
        val filterRust = CoreEngineFacade.Conversation.filterHistoryViaRust(pruneRust.history)

        val coalesceLocal = CoreEngineFacade.Conversation.coalesceSnapshotLocally(request.snapshotRecords)
        val coalesceRust = CoreEngineFacade.Conversation.coalesceSnapshotViaRust(request.snapshotRecords)

        return AutoDriveTurnFlowResultPayload(
            autoSequence = AutoDriveTurnFlowStagePayload(local = autoSequenceLocal, rust = autoSequenceRust),
            pruneHistory = ConversationPruneStagePayload(local = pruneLocal, rust = pruneRust),
            filterHistory = ConversationFilterStagePayload(local = filterLocal, rust = filterRust),
            coalesceSnapshot = ConversationCoalesceStagePayload(local = coalesceLocal, rust = coalesceRust),
        )
    }
}

@Serializable
data class AutoDriveTurnFlowRequest(
    @SerialName("initial_state") val initialState: AutoDriveControllerStatePayload,
    val operations: List<AutoDriveSequenceOperationPayload>,
    val history: List<JsonElement>,
    @SerialName("drop_last_user_turns") val dropLastUserTurns: Int = 0,
    @SerialName("snapshot_records") val snapshotRecords: List<HistorySnapshotRecordPayload>,
)

@Serializable
data class AutoDriveTurnFlowResultPayload(
    @SerialName("auto_sequence") val autoSequence: AutoDriveTurnFlowStagePayload,
    @SerialName("prune_history") val pruneHistory: ConversationPruneStagePayload,
    @SerialName("filter_history") val filterHistory: ConversationFilterStagePayload,
    @SerialName("coalesce_snapshot") val coalesceSnapshot: ConversationCoalesceStagePayload,
)

@Serializable
data class AutoDriveTurnFlowStagePayload(
    val local: AutoDriveSequenceResponse,
    val rust: AutoDriveSequenceResponse,
)

@Serializable
data class ConversationPruneStagePayload(
    val local: ConversationPruneHistoryResponse,
    val rust: ConversationPruneHistoryResponse,
)

@Serializable
data class ConversationFilterStagePayload(
    val local: ConversationFilterHistoryResponse,
    val rust: ConversationFilterHistoryResponse,
)

@Serializable
data class ConversationCoalesceStagePayload(
    val local: ConversationCoalesceSnapshotResponse,
    val rust: ConversationCoalesceSnapshotResponse,
)
