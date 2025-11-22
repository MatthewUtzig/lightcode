package ai.lightcode.core.engine

/**
 * Kotlin mirror of the snapshot coalescing logic used when Rust rebuilds
 * `HistorySnapshot`s.
 */
object HistorySnapshotControllerKt {

    fun coalesce(records: List<HistorySnapshotRecordPayload>): ConversationCoalesceSnapshotResponse {
        var removed = 0
        val seenStreams = mutableSetOf<String>()
        val retained = buildList {
            records.forEach { record ->
                val shouldSkip = record.kind == HistorySnapshotRecordKindPayload.ASSISTANT &&
                    !record.streamId.isNullOrEmpty() &&
                    !seenStreams.add(record.streamId)
                if (shouldSkip) {
                    removed += 1
                } else {
                    add(record)
                }
            }
        }

        return ConversationCoalesceSnapshotResponse(
            status = "ok",
            kind = "conversation_coalesce_snapshot",
            records = retained,
            removedCount = removed,
        )
    }

    fun summarize(records: List<HistorySnapshotRecordPayload>): ConversationSnapshotSummaryResponse {
        val assistant = records.count { it.kind == HistorySnapshotRecordKindPayload.ASSISTANT }
        val user = records.count { it.kind == HistorySnapshotRecordKindPayload.USER }
        return ConversationSnapshotSummaryResponse(
            status = "ok",
            kind = "conversation_snapshot_summary",
            recordCount = records.size,
            assistantMessages = assistant,
            userMessages = user,
        )
    }
}
