package ai.lightcode.core.engine.coordinator

import ai.lightcode.core.engine.CoreEngineApi
import ai.lightcode.core.engine.SimpleModelTurnRequest
import ai.lightcode.core.engine.SimpleModelTurnResponse
import ai.lightcode.core.engine.TokenUsagePayload
import kotlinx.serialization.json.JsonElement

/**
 * Minimal coordinator abstraction that lets Kotlin own a slice of the
 * auto-drive decision loop without changing the Rust protocol yet.
 */
data class KotlinCoordinatorInput(
    val history: List<JsonElement>,
    val latestUserPrompt: String?,
)

data class KotlinCoordinatorResult(
    val decisions: List<KotlinCoordinatorDecision>,
    val tokenMetrics: KotlinTokenMetrics? = null,
    val fallbackReason: String? = null,
) {
    val isUsable: Boolean
        get() = fallbackReason == null && decisions.isNotEmpty()
}

sealed interface KotlinCoordinatorDecision {
    data class Thinking(val text: String, val summaryIndex: Int?) : KotlinCoordinatorDecision
    data class FinalAnswer(val text: String) : KotlinCoordinatorDecision
    data class RequestExecCommand(
        val command: String,
        val preview: String,
        val rationale: String?,
    ) : KotlinCoordinatorDecision
    data class RequestApplyPatch(
        val patch: String,
        val preview: String,
        val rationale: String?,
    ) : KotlinCoordinatorDecision
    object StopAcknowledged : KotlinCoordinatorDecision
}

fun interface SimpleModelTurnRunner {
    fun invoke(request: SimpleModelTurnRequest): SimpleModelTurnResponse
}

interface KotlinCoordinator {
    fun runTurn(input: KotlinCoordinatorInput): KotlinCoordinatorResult
}

class SimpleKotlinCoordinator(
    private val runner: SimpleModelTurnRunner = SimpleModelTurnRunner { request ->
        CoreEngineApi.simpleModelTurn(request)
    },
) : KotlinCoordinator {

    override fun runTurn(input: KotlinCoordinatorInput): KotlinCoordinatorResult {
        val response = kotlin.runCatching {
            runner.invoke(
                SimpleModelTurnRequest(
                    history = input.history,
                    latestUserPrompt = input.latestUserPrompt,
                ),
            )
        }.getOrElse { throwable ->
            return KotlinCoordinatorResult(
                decisions = emptyList(),
                fallbackReason = throwable.message ?: "simple_model_turn failed",
            )
        }

        if (response.status != "ok") {
            return KotlinCoordinatorResult(
                decisions = emptyList(),
                fallbackReason = response.message ?: "simple_model_turn status ${response.status}",
            )
        }

        val decisions = mutableListOf<KotlinCoordinatorDecision>()
        response.thinking.mapIndexedNotNull { index, raw ->
            val trimmed = raw.trim()
            trimmed.takeIf { it.isNotEmpty() }?.let {
                KotlinCoordinatorDecision.Thinking(text = it, summaryIndex = index)
            }
        }.forEach(decisions::add)

        val answer = response.answer.trim()
        if (answer.isEmpty()) {
            return KotlinCoordinatorResult(decisions = decisions)
        }

        decisions.add(KotlinCoordinatorDecision.FinalAnswer(answer))
        detectExecCommands(answer).map {
            KotlinCoordinatorDecision.RequestExecCommand(
                command = it.full,
                preview = it.preview,
                rationale = "Detected shell fence from Kotlin coordinator",
            )
        }.forEach(decisions::add)
        detectPatchBlocks(answer).map {
            KotlinCoordinatorDecision.RequestApplyPatch(
                patch = it.full,
                preview = it.preview,
                rationale = "Detected patch fence from Kotlin coordinator",
            )
        }.forEach(decisions::add)

        val tokenMetrics = buildTokenMetrics(response)
        return KotlinCoordinatorResult(decisions = decisions, tokenMetrics = tokenMetrics)
    }

    private fun detectExecCommands(answer: String): List<CodeFence> {
        return CODE_FENCE_REGEX.findAll(answer)
            .mapNotNull { match ->
                val language = match.groups[1]?.value?.lowercase()
                val body = match.groups[2]?.value?.trim()?.takeIf { it.isNotEmpty() }
                val supported = language in setOf("sh", "bash", "zsh", "shell")
                body?.takeIf { supported }?.let { CodeFence(it, firstLine(it)) }
            }
            .distinctBy { it.full }
            .toList()
    }

    private fun detectPatchBlocks(answer: String): List<CodeFence> {
        return PATCH_FENCE_REGEX.findAll(answer)
            .mapNotNull { match ->
                match.groups[2]?.value?.trim()?.takeIf { it.isNotEmpty() }
            }
            .map { CodeFence(it, firstLine(it)) }
            .distinctBy { it.full }
            .toList()
    }

    private fun firstLine(block: String): String {
        return block.lines().firstOrNull()?.trim().takeUnless { it.isNullOrEmpty() } ?: block.trim()
    }

    private data class CodeFence(val full: String, val preview: String)

    companion object {
        private val CODE_FENCE_REGEX = Regex("```(\\w+)[\\r\\n]+([\\s\\S]*?)```", RegexOption.IGNORE_CASE)
        private val PATCH_FENCE_REGEX = Regex("```(diff|patch)[\\r\\n]+([\\s\\S]*?)```", RegexOption.IGNORE_CASE)
    }

    private fun buildTokenMetrics(response: SimpleModelTurnResponse): KotlinTokenMetrics? {
        val usage = response.tokenUsage ?: return null
        val mapped = usage.toKotlinTokenUsage()
        return KotlinTokenMetrics(
            totalUsage = mapped,
            lastTurnUsage = mapped,
            turnCount = 1,
            duplicateItems = 0,
            replayUpdates = 0,
        )
    }

    private fun TokenUsagePayload.toKotlinTokenUsage() = KotlinTokenUsage(
        inputTokens = inputTokens,
        cachedInputTokens = cachedInputTokens,
        outputTokens = outputTokens,
        reasoningOutputTokens = reasoningOutputTokens,
        totalTokens = totalTokens,
    )
}
data class KotlinTokenMetrics(
    val totalUsage: KotlinTokenUsage,
    val lastTurnUsage: KotlinTokenUsage,
    val turnCount: Int,
    val duplicateItems: Int,
    val replayUpdates: Int,
)

data class KotlinTokenUsage(
    val inputTokens: Int,
    val cachedInputTokens: Int,
    val outputTokens: Int,
    val reasoningOutputTokens: Int,
    val totalTokens: Int,
)
