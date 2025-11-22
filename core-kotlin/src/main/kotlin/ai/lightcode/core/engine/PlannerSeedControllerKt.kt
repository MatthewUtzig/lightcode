package ai.lightcode.core.engine

object PlannerSeedControllerKt {

    fun buildSeed(goalText: String, includeAgents: Boolean): AutoCoordinatorPlanningSeedResponse {
        val goal = goalText.trim()
        if (goal.isEmpty()) {
            return AutoCoordinatorPlanningSeedResponse(
                status = "ok",
                kind = "auto_coordinator_planning_seed",
                responseJson = null,
            )
        }

        val cliPrompt = if (includeAgents) {
            "Please provide a clear plan to best achieve the Primary Goal. If this is not a trival task, launch agents and use your tools to research the best approach. If this is a trival task, or the plan is already in the conversation history, just imediately provide the plan. Judge the length of research and planning you perform based on the complexity of the task. For more complex tasks, you could break the plan into workstreams which can be performed at the same time."
        } else {
            "Please provide a clear plan to best achieve the Primary Goal. If this is not a trival task, use your tools to research the best approach. If this is a trival task, or the plan is already in the conversation history, just imediately provide the plan. Judge the length of research and planning you perform based on the complexity of the task."
        }

        val responseJson = "{\"finish_status\":\"continue\",\"status_title\":\"Planning\",\"status_sent_to_user\":\"Started initial planning phase\",\"prompt_sent_to_cli\":\"${cliPrompt.replace("\"", "\\\"")}\"}"

        return AutoCoordinatorPlanningSeedResponse(
            status = "ok",
            kind = "auto_coordinator_planning_seed",
            responseJson = responseJson,
            cliPrompt = cliPrompt,
            goalMessage = "Primary Goal: $goal",
            statusTitle = "Planning route",
            statusSentToUser = "Planning best route to reach the goal.",
            agentsTiming = if (includeAgents) AutoTurnAgentsTimingPayload.PARALLEL else null,
        )
    }
}
