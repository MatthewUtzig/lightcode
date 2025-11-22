package ai.lightcode.core.engine.coordinator

import ai.lightcode.core.engine.SimpleModelTurnRequest
import ai.lightcode.core.engine.SimpleModelTurnResponse
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.jupiter.api.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class KotlinCoordinatorTest {

    @Test
    fun emitsThinkingAndAnswerDecisions() {
        val recordedRequests = mutableListOf<SimpleModelTurnRequest>()
        val coordinator = SimpleKotlinCoordinator(
            runner = SimpleModelTurnRunner { request ->
                recordedRequests.add(request)
                SimpleModelTurnResponse(
                    status = "ok",
                    kind = "simple_model_turn",
                    thinking = listOf("ponder", "decide"),
                    answer = "```bash\necho run-tests\n```\n\n```diff\n--- a/file\n+++ b/file\n@@\n+line\n```",
                )
            },
        )
        val result = coordinator.runTurn(
            KotlinCoordinatorInput(
                history = listOf(buildJsonObject { put("type", "message") }),
                latestUserPrompt = "hello",
            ),
        )

        assertTrue(result.isUsable)
        val thinking = result.decisions.filterIsInstance<KotlinCoordinatorDecision.Thinking>()
        assertEquals(listOf("ponder", "decide"), thinking.map { it.text })
        val answers = result.decisions.filterIsInstance<KotlinCoordinatorDecision.FinalAnswer>()
        assertEquals(1, answers.size)
    }

    @Test
    fun detectsExecAndPatchRequests() {
        val coordinator = SimpleKotlinCoordinator(
            runner = SimpleModelTurnRunner {
                SimpleModelTurnResponse(
                    status = "ok",
                    kind = "simple_model_turn",
                    thinking = listOf("ponder"),
                    answer = "```bash\necho run-tests\n```\n\n```diff\n--- a/file\n+++ b/file\n@@\n+line\n```",
                )
            },
        )
        val result = coordinator.runTurn(
            KotlinCoordinatorInput(history = emptyList(), latestUserPrompt = "run something"),
        )

        val execDecisions = result.decisions.filterIsInstance<KotlinCoordinatorDecision.RequestExecCommand>()
        assertEquals(listOf("echo run-tests"), execDecisions.map { it.preview })
        assertTrue(execDecisions.first().command.contains("echo run-tests"))

        val patchDecisions = result.decisions.filterIsInstance<KotlinCoordinatorDecision.RequestApplyPatch>()
        assertEquals(1, patchDecisions.size)
        assertTrue(patchDecisions.first().patch.contains("a/file"))
    }

    @Test
    fun recordsHistoryInRunnerRequest() {
        val recordedRequests = mutableListOf<SimpleModelTurnRequest>()
        val coordinator = SimpleKotlinCoordinator(
            runner = SimpleModelTurnRunner { request ->
                recordedRequests.add(request)
                SimpleModelTurnResponse(
                    status = "ok",
                    kind = "simple_model_turn",
                    thinking = emptyList(),
                    answer = "Done",
                )
            },
        )
        val history = listOf(buildJsonObject { put("role", "user") })
        coordinator.runTurn(KotlinCoordinatorInput(history = history, latestUserPrompt = null))
        assertTrue(recordedRequests.first().history.isNotEmpty())
    }
}
