package ai.lightcode.core.engine

import kotlinx.serialization.json.JsonElement

/**
 * High-level façade that exposes Kotlin mirrors of selected Core Engine
 * features while also providing access to the authoritative Rust
 * implementations via JNI. Kotlin callers can stick to this API without
 * juggling the individual helper objects.
 */
object CoreEngineFacade {

    object AutoDrive {
        fun runSequenceLocally(
            initialState: AutoDriveControllerStatePayload,
            operations: List<AutoDriveSequenceOperationPayload>,
        ): AutoDriveSequenceResponse {
            val controller = KotlinAutoDriveControllerState(initialState)
            val steps = operations.map { controller.apply(it) }
            return AutoDriveSequenceResponse(
                status = "ok",
                kind = "auto_drive_sequence",
                steps = steps,
            )
        }

        fun runSequenceViaRust(
            initialState: AutoDriveControllerStatePayload,
            operations: List<AutoDriveSequenceOperationPayload>,
        ): AutoDriveSequenceResponse =
            CoreEngineApi.autoDriveSequence(initialState, operations)
    }

    object Conversation {
        fun pruneHistoryLocally(
            history: List<JsonElement>,
            dropLastUserTurns: Int,
        ): ConversationPruneHistoryResponse =
            ConversationControllerKt.pruneHistory(history, dropLastUserTurns)

        fun pruneHistoryViaRust(
            history: List<JsonElement>,
            dropLastUserTurns: Int,
        ): ConversationPruneHistoryResponse =
            CoreEngineApi.conversationPruneHistory(history, dropLastUserTurns)

        fun filterHistoryLocally(history: List<JsonElement>): ConversationFilterHistoryResponse =
            ConversationControllerKt.filterHistory(history)

        fun filterHistoryViaRust(history: List<JsonElement>): ConversationFilterHistoryResponse =
            CoreEngineApi.conversationFilterHistory(history)

        fun coalesceSnapshotLocally(
            records: List<HistorySnapshotRecordPayload>,
        ): ConversationCoalesceSnapshotResponse =
            HistorySnapshotControllerKt.coalesce(records)

        fun coalesceSnapshotViaRust(
            records: List<HistorySnapshotRecordPayload>,
        ): ConversationCoalesceSnapshotResponse =
            CoreEngineApi.conversationCoalesceSnapshot(records)

        fun snapshotSummaryLocally(
            records: List<HistorySnapshotRecordPayload>,
        ): ConversationSnapshotSummaryResponse =
            HistorySnapshotControllerKt.summarize(records)

        fun snapshotSummaryViaRust(
            records: List<HistorySnapshotRecordPayload>,
        ): ConversationSnapshotSummaryResponse =
            CoreEngineApi.conversationSnapshotSummary(records)

        fun forkHistoryLocally(
            history: List<JsonElement>,
            dropLastUserTurns: Int,
        ): ConversationForkHistoryResponse =
            ConversationControllerKt.forkHistory(history, dropLastUserTurns)

        fun forkHistoryViaRust(
            history: List<JsonElement>,
            dropLastUserTurns: Int,
        ): ConversationForkHistoryResponse =
            CoreEngineApi.conversationForkHistory(history, dropLastUserTurns)

        fun filterPopularCommandsLocally(
            history: List<JsonElement>,
        ): ConversationFilterPopularCommandsResponse =
            ConversationControllerKt.filterPopularCommands(history)

        fun filterPopularCommandsViaRust(
            history: List<JsonElement>,
        ): ConversationFilterPopularCommandsResponse =
            CoreEngineApi.conversationFilterPopularCommands(history)

        fun planningSeedLocally(
            goalText: String,
            includeAgents: Boolean,
        ): AutoCoordinatorPlanningSeedResponse =
            PlannerSeedControllerKt.buildSeed(goalText, includeAgents)

        fun planningSeedViaRust(
            goalText: String,
            includeAgents: Boolean,
        ): AutoCoordinatorPlanningSeedResponse =
            CoreEngineApi.autoCoordinatorPlanningSeed(goalText, includeAgents)
    }
}
