package ai.lightcode.core.engine

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonPrimitive

/**
 * Kotlin-side mirrors of lightweight conversation operations so we can keep
 * behaviour parity with the Rust conversation manager.
 */
object ConversationControllerKt {

    fun pruneHistory(
        history: List<JsonElement>,
        dropLastUserTurns: Int,
    ): ConversationPruneHistoryResponse {
        require(dropLastUserTurns >= 0) { "dropLastUserTurns must be non-negative" }

        if (dropLastUserTurns == 0) {
            return response(history, prunedTurns = 0, wasReset = false)
        }

        val userPositions = history.mapIndexedNotNull { index, element ->
            if (element.isUserMessage()) index else null
        }

        if (userPositions.isEmpty()) {
            return response(emptyList(), prunedTurns = 0, wasReset = true)
        }

        if (userPositions.size < dropLastUserTurns) {
            return response(emptyList(), prunedTurns = userPositions.size, wasReset = true)
        }

        val cutIndex = userPositions[userPositions.size - dropLastUserTurns]
        val retainedHistory = history.take(cutIndex)
        val wasReset = retainedHistory.isEmpty()

        return response(retainedHistory, prunedTurns = dropLastUserTurns, wasReset = wasReset)
    }

    fun filterHistory(history: List<JsonElement>): ConversationFilterHistoryResponse {
        val filtered = history.filter { it.isApiMessage() }
        val removed = history.size - filtered.size
        return ConversationFilterHistoryResponse(
            status = "ok",
            kind = "conversation_filter_history",
            history = filtered,
            removedCount = removed,
        )
    }

    fun forkHistory(
        history: List<JsonElement>,
        dropLastUserTurns: Int,
    ): ConversationForkHistoryResponse {
        require(dropLastUserTurns >= 0) { "dropLastUserTurns must be non-negative" }

        val userPositions = history.mapIndexedNotNull { index, element ->
            if (element.isUserMessage()) index else null
        }

        if (dropLastUserTurns == 0) {
            return ConversationForkHistoryResponse(
                status = "ok",
                kind = "conversation_fork_history",
                history = history,
                droppedUserTurns = 0,
                becameNew = false,
            )
        }

        if (userPositions.isEmpty() || userPositions.size < dropLastUserTurns) {
            return ConversationForkHistoryResponse(
                status = "ok",
                kind = "conversation_fork_history",
                history = emptyList(),
                droppedUserTurns = userPositions.size,
                becameNew = true,
            )
        }

        val cutIndex = userPositions[userPositions.size - dropLastUserTurns]
        val retainedHistory = history.take(cutIndex)
        return ConversationForkHistoryResponse(
            status = "ok",
            kind = "conversation_fork_history",
            history = retainedHistory,
            droppedUserTurns = dropLastUserTurns,
            becameNew = retainedHistory.isEmpty(),
        )
    }

    fun filterPopularCommands(
        history: List<JsonElement>,
    ): ConversationFilterPopularCommandsResponse {
        val filtered = history.filterNot { it.isPopularCommandsMessage() }
        return ConversationFilterPopularCommandsResponse(
            status = "ok",
            kind = "conversation_filter_popular_commands",
            history = filtered,
        )
    }

    fun planningSeed(
        goalText: String,
        includeAgents: Boolean,
    ): AutoCoordinatorPlanningSeedResponse =
        PlannerSeedControllerKt.buildSeed(goalText, includeAgents)

    private fun response(
        history: List<JsonElement>,
        prunedTurns: Int,
        wasReset: Boolean,
    ): ConversationPruneHistoryResponse =
        ConversationPruneHistoryResponse(
            status = "ok",
            kind = "conversation_prune_history",
            history = history,
            prunedUserTurns = prunedTurns,
            wasReset = wasReset,
        )

    private fun JsonElement.isUserMessage(): Boolean {
        val obj = this as? JsonObject ?: return false
        val type = obj["type"]?.jsonPrimitive?.contentOrNull
        if (type != "message") {
            return false
        }
        val role = obj["role"]?.jsonPrimitive?.contentOrNull
        return role == "user"
    }

    private fun JsonElement.isApiMessage(): Boolean {
        val obj = this as? JsonObject ?: return false
        val type = obj["type"]?.jsonPrimitive?.contentOrNull ?: return false
        if (type == "message") {
            val role = obj["role"]?.jsonPrimitive?.contentOrNull
            return role != "system"
        }
        return type != "other"
    }

    private fun JsonElement.isPopularCommandsMessage(): Boolean {
        val obj = this as? JsonObject ?: return false
        val type = obj["type"]?.jsonPrimitive?.contentOrNull
        if (type != "message") {
            return false
        }
        val role = obj["role"]?.jsonPrimitive?.contentOrNull
        if (!role.equals("user", ignoreCase = true)) {
            return false
        }
        val content = obj["content"] as? kotlinx.serialization.json.JsonArray ?: return false
        return content.any { element ->
            val item = element as? JsonObject ?: return@any false
            val itemType = item["type"]?.jsonPrimitive?.contentOrNull
            val text = item["text"]?.jsonPrimitive?.contentOrNull
            itemType == "input_text" && text?.contains("Popular commands:") == true
        }
    }
}
