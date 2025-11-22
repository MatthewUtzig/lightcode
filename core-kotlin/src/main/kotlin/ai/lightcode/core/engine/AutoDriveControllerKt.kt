package ai.lightcode.core.engine

import java.util.UUID

internal const val AUTO_RESTART_MAX_ATTEMPTS = 6
private const val AUTO_RESTART_BASE_DELAY_MS = 5_000L
private const val AUTO_RESTART_MAX_DELAY_MS = 120_000L
private const val MAX_REASON_LENGTH = 160

/**
 * Kotlin-side mirrors of specific Auto Drive controller operations so we can
 * compare behaviour against the Rust implementation without touching the live
 * UI yet.
 */
object AutoDriveControllerKt {

    fun handleCountdownTick(request: AutoDriveCountdownTickRequest): AutoDriveCountdownTickResponse {
        val shouldEmit = request.phase.shouldEmitCountdownEffect()
        val effects = buildList {
            if (shouldEmit) {
                if (request.secondsLeft == 0) {
                    add(AutoDriveEffectPayload(type = "submit_prompt"))
                } else {
                    add(AutoDriveEffectPayload(type = "refresh_ui"))
                }
            }
        }
        val reportedSeconds = if (shouldEmit) request.secondsLeft else 0

        return AutoDriveCountdownTickResponse(
            status = "ok",
            kind = "auto_drive_countdown_tick",
            effects = effects,
            secondsLeft = reportedSeconds,
        )
    }

    fun updateContinueMode(request: AutoDriveUpdateContinueModeRequest): AutoDriveUpdateContinueModeResponse {
        val seconds = request.continueMode.seconds()
        val secondsRemaining = seconds ?: 0
        val shouldStartCountdown = request.phase.shouldEmitCountdownEffect() && seconds != null
        val newCountdownId = if (shouldStartCountdown) request.countdownId + 1 else request.countdownId
        val effectiveDecisionSeq = if (secondsRemaining == 0) 0 else request.decisionSeq

        val effects = buildList {
            add(AutoDriveEffectPayload(type = "refresh_ui"))
            if (shouldStartCountdown) {
                add(
                    AutoDriveEffectPayload(
                        type = "start_countdown",
                        countdownId = newCountdownId,
                        decisionSeq = effectiveDecisionSeq,
                        seconds = seconds,
                    ),
                )
            }
        }

        return AutoDriveUpdateContinueModeResponse(
            status = "ok",
            kind = "auto_drive_update_continue_mode",
            effects = effects,
            secondsLeft = secondsRemaining,
        )
    }
}

class KotlinAutoDriveControllerState(
    initialState: AutoDriveControllerStatePayload,
) {
    private var phase: AutoRunPhasePayload = initialState.phase
    private var continueMode: AutoContinueModePayload = initialState.continueMode
    private var countdownId: Long = initialState.countdownId
    private var countdownDecisionSeq: Long = initialState.countdownDecisionSeq
    private var secondsRemaining: Int = continueMode.seconds() ?: 0
    private var transientRestartAttempts: Int = 0
    private var restartToken: Long = 0
    private var goal: String? = null
    private var turnsCompleted: Int = 0
    private var lastStopMessage: String? = null
    private var execRequestIssued: Boolean = false
    private var patchRequestIssued: Boolean = false

    fun apply(operation: AutoDriveSequenceOperationPayload): AutoDriveSequenceStepPayload {
        val effects = when (operation) {
            is AutoDriveSequenceOperationPayload.UpdateContinueMode -> applyUpdateContinueMode(operation.mode)
            is AutoDriveSequenceOperationPayload.HandleCountdownTick -> applyCountdownTick(operation)
            is AutoDriveSequenceOperationPayload.PauseForTransientFailure -> applyPauseForTransientFailure(operation.reason)
            is AutoDriveSequenceOperationPayload.StopRun -> applyStopRun(operation.message)
            is AutoDriveSequenceOperationPayload.LaunchResult -> applyLaunchResult(operation)
        }

        return AutoDriveSequenceStepPayload(
            effects = effects,
            snapshot = snapshot(),
        )
    }

    private fun applyUpdateContinueMode(mode: AutoContinueModePayload): List<AutoDriveEffectPayload> {
        continueMode = mode
        secondsRemaining = mode.seconds() ?: 0
        if (secondsRemaining == 0) {
            countdownDecisionSeq = 0
        }
        val shouldStartCountdown = phase.shouldEmitCountdownEffect() && mode.seconds() != null
        val decisionSeq = if (secondsRemaining == 0) 0 else countdownDecisionSeq

        if (shouldStartCountdown) {
            countdownId += 1
        }

        return buildList {
            add(AutoDriveEffectPayload(type = "refresh_ui"))
            if (shouldStartCountdown) {
                add(
                    AutoDriveEffectPayload(
                        type = "start_countdown",
                        countdownId = countdownId,
                        decisionSeq = decisionSeq,
                        seconds = mode.seconds(),
                    ),
                )
            }
        }
    }

    private fun applyCountdownTick(op: AutoDriveSequenceOperationPayload.HandleCountdownTick): List<AutoDriveEffectPayload> {
        val shouldEmit = phase.shouldEmitCountdownEffect() &&
            op.countdownId == countdownId &&
            op.decisionSeq == countdownDecisionSeq

        if (!shouldEmit) {
            return emptyList()
        }

        secondsRemaining = op.secondsLeft
        val type = if (op.secondsLeft == 0) "submit_prompt" else "refresh_ui"
        return listOf(AutoDriveEffectPayload(type = type))
    }

    private fun applyPauseForTransientFailure(reason: String): List<AutoDriveEffectPayload> {
        val pendingAttempt = (transientRestartAttempts + 1).coerceAtMost(Int.MAX_VALUE)
        val truncatedReason = truncateReason(reason)

        if (pendingAttempt > AUTO_RESTART_MAX_ATTEMPTS) {
            phase = AutoRunPhasePayload.awaitingGoalEntry()
            countdownId = 0
            countdownDecisionSeq = 0
            secondsRemaining = continueMode.seconds() ?: 0
            transientRestartAttempts = 0
            restartToken = 0
            val stopMessage =
                "Auto Drive stopped after $AUTO_RESTART_MAX_ATTEMPTS reconnect attempts. Last error: $truncatedReason"

            return listOf(
                AutoDriveEffectPayload(type = "cancel_coordinator"),
                AutoDriveEffectPayload(type = "reset_history"),
                AutoDriveEffectPayload(type = "clear_coordinator_view"),
                AutoDriveEffectPayload(type = "update_terminal_hint", hint = null),
                AutoDriveEffectPayload(type = "set_task_running", running = false),
                AutoDriveEffectPayload(type = "ensure_input_focus"),
                AutoDriveEffectPayload(
                    type = "stop_completed",
                    message = stopMessage,
                    turnsCompleted = 0,
                    durationMs = 0,
                ),
                AutoDriveEffectPayload(type = "refresh_ui"),
            )
        }

        transientRestartAttempts = pendingAttempt
        val delayMs = autoRestartDelayMs(pendingAttempt)
        phase = AutoRunPhasePayload.transientRecovery(backoffMs = delayMs)
        restartToken += 1

        return listOf(
            AutoDriveEffectPayload(type = "cancel_coordinator"),
            AutoDriveEffectPayload(type = "set_task_running", running = false),
            AutoDriveEffectPayload(
                type = "update_terminal_hint",
                hint = "Press Esc again to exit Auto Drive",
            ),
            AutoDriveEffectPayload(
                type = "transient_pause",
                attempt = pendingAttempt,
                delayMs = delayMs,
                reason = truncatedReason,
            ),
            AutoDriveEffectPayload(
                type = "schedule_restart",
                token = restartToken,
                attempt = pendingAttempt,
                delayMs = delayMs,
            ),
            AutoDriveEffectPayload(type = "refresh_ui"),
        )
    }

    private fun applyStopRun(message: String?): List<AutoDriveEffectPayload> {
        lastStopMessage = message
        phase = AutoRunPhasePayload.awaitingGoalEntry()
        countdownId = 0
        countdownDecisionSeq = 0
        secondsRemaining = continueMode.seconds() ?: 0
        execRequestIssued = false
        patchRequestIssued = false
        val summary = AutoDriveEffectPayload(
            type = "stop_completed",
            message = message,
            turnsCompleted = turnsCompleted,
            durationMs = 0,
        )
        return buildList {
            add(AutoDriveEffectPayload(type = "cancel_coordinator"))
            add(AutoDriveEffectPayload(type = "reset_history"))
            add(AutoDriveEffectPayload(type = "clear_coordinator_view"))
            add(AutoDriveEffectPayload(type = "update_terminal_hint", hint = null))
            add(AutoDriveEffectPayload(type = "set_task_running", running = false))
            add(AutoDriveEffectPayload(type = "ensure_input_focus"))
            add(summary)
            maybeBuildPatchRequestEffect()?.let { add(it) }
            add(AutoDriveEffectPayload(type = "refresh_ui"))
        }
    }

    private fun applyLaunchResult(operation: AutoDriveSequenceOperationPayload.LaunchResult): List<AutoDriveEffectPayload> {
        return when (operation.result) {
            AutoDriveSequenceOperationPayload.LaunchOutcome.SUCCEEDED -> {
                phase = AutoRunPhasePayload.awaitingDiagnostics(coordinatorWaiting = true)
                goal = operation.goal
                val effects = mutableListOf(
                    AutoDriveEffectPayload(type = "launch_started", message = operation.goal),
                    AutoDriveEffectPayload(type = "refresh_ui"),
                )
                maybeBuildExecRequestEffect()?.let { effects.add(it) }
                effects
            }
            AutoDriveSequenceOperationPayload.LaunchOutcome.FAILED -> {
                phase = AutoRunPhasePayload.awaitingGoalEntry()
                listOf(
                    AutoDriveEffectPayload(
                        type = "launch_failed",
                        message = operation.goal,
                        hint = operation.error,
                    ),
                    AutoDriveEffectPayload(type = "show_goal_entry"),
                    AutoDriveEffectPayload(type = "refresh_ui"),
                )
            }
        }
    }

    private fun maybeBuildExecRequestEffect(): AutoDriveEffectPayload? {
        if (execRequestIssued) {
            return null
        }
        execRequestIssued = true
        val effectiveGoal = goal?.takeIf { it.isNotBlank() }
        val reason = effectiveGoal?.let { truncateReason("Run Kotlin exec for $it") }
            ?: "Run Kotlin exec for current Auto Drive step"
        return AutoDriveEffectPayload(
            type = "kotlin_exec_request",
            execRequest = AutoDriveExecRequestPayload(
                callId = "kexec-${UUID.randomUUID()}",
                command = listOf("bash", "-lc", "echo Kotlin exec pipeline ready"),
                cwd = null,
                reason = truncateReason(reason),
            ),
        )
    }

    private fun maybeBuildPatchRequestEffect(): AutoDriveEffectPayload? {
        if (patchRequestIssued) {
            return null
        }
        patchRequestIssued = true
        val effectiveGoal = goal?.takeIf { it.isNotBlank() } ?: "current Auto Drive step"
        val summary = truncateReason("Apply Kotlin patch for $effectiveGoal")
        val trimmedGoal = if (effectiveGoal.length <= 60) {
            effectiveGoal
        } else {
            effectiveGoal.substring(0, 60) + "…"
        }
        val body = buildString {
            appendLine("# Kotlin Patch Note")
            appendLine("Generated for $trimmedGoal")
        }
        return AutoDriveEffectPayload(
            type = "kotlin_patch_request",
            patchRequest = AutoDrivePatchRequestPayload(
                callId = "kpatch-${UUID.randomUUID()}",
                changes = mapOf(
                    "KOTLIN_PATCH_NOTE.md" to AutoDrivePatchChangePayload(
                        kind = AutoDrivePatchChangeKind.Add,
                        content = body,
                        unifiedDiff = null,
                        movePath = null,
                    ),
                ),
                reason = summary,
                grantRoot = null,
            ),
        )
    }

    private fun snapshot(): AutoDriveControllerSnapshotPayload =
        AutoDriveControllerSnapshotPayload(
            phase = phase,
            continueMode = continueMode,
            countdownId = countdownId,
            countdownDecisionSeq = countdownDecisionSeq,
            secondsRemaining = secondsRemaining,
            transientRestartAttempts = transientRestartAttempts,
            restartToken = restartToken,
        )
}

private fun AutoRunPhasePayload.shouldEmitCountdownEffect(): Boolean =
    isActivePhase() && !isPausedManualPhase() && awaitingCoordinatorSubmit()

private fun AutoRunPhasePayload.isActivePhase(): Boolean =
    when (name) {
        AutoRunPhaseName.IDLE,
        AutoRunPhaseName.AWAITING_GOAL_ENTRY -> false
        AutoRunPhaseName.LAUNCHING,
        AutoRunPhaseName.ACTIVE,
        AutoRunPhaseName.PAUSED_MANUAL,
        AutoRunPhaseName.AWAITING_COORDINATOR,
        AutoRunPhaseName.AWAITING_DIAGNOSTICS,
        AutoRunPhaseName.AWAITING_REVIEW,
        AutoRunPhaseName.TRANSIENT_RECOVERY -> true
    }

private fun AutoRunPhasePayload.isPausedManualPhase(): Boolean =
    name == AutoRunPhaseName.PAUSED_MANUAL

private fun AutoRunPhasePayload.awaitingCoordinatorSubmit(): Boolean =
    name == AutoRunPhaseName.AWAITING_COORDINATOR && promptReady == true

private fun AutoContinueModePayload.seconds(): Int? =
    when (this) {
        AutoContinueModePayload.IMMEDIATE -> 0
        AutoContinueModePayload.TEN_SECONDS -> 10
        AutoContinueModePayload.SIXTY_SECONDS -> 60
        AutoContinueModePayload.MANUAL -> null
    }

private fun autoRestartDelayMs(attempt: Int): Long {
    if (attempt <= 1) {
        return AUTO_RESTART_BASE_DELAY_MS.coerceAtMost(AUTO_RESTART_MAX_DELAY_MS)
    }
    val exponent = (attempt - 1).coerceAtMost(5)
    val multiplier = 1L shl exponent
    val delay = AUTO_RESTART_BASE_DELAY_MS * multiplier
    return delay.coerceAtMost(AUTO_RESTART_MAX_DELAY_MS)
}

private fun truncateReason(reason: String): String {
    val trimmed = reason.trim()
    if (trimmed.length <= MAX_REASON_LENGTH) {
        return trimmed
    }
    return trimmed.take(MAX_REASON_LENGTH) + "…"
}
