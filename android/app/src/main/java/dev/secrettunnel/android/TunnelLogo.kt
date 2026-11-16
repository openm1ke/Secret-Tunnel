package dev.secrettunnel.android

import android.animation.ValueAnimator
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RadialGradient
import android.graphics.Shader
import android.view.View
import android.view.animation.LinearInterpolator

internal class TunnelLogo(context: Context) : View(context) {
    enum class State { OFF, CONNECTING, ON }

    private var state = State.OFF
    private var highlighted = 0
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
    }
    private val dot = Paint(Paint.ANTI_ALIAS_FLAG)
    private val glow = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        shader = RadialGradient(256f, 386f, 86f,
            intArrayOf(0xffffd27a.toInt(), 0x8cf2a541.toInt(), 0x00f2a541),
            floatArrayOf(0f, .45f, 1f), Shader.TileMode.CLAMP)
    }

    // Match the three arches and colors of the desktop tray artwork in design/tray.
    private class Arch(val left: Float, val spring: Float, val width: Float, off: String, on: String) {
        val offColor = Color.parseColor(off)
        val onColor = Color.parseColor(on)
        val path = Path().apply {
            val right = 512f - left
            val radius = 256f - left
            moveTo(left, 436f)
            lineTo(left, spring)
            arcTo(left, spring - radius, right, spring + radius, 180f, 180f, false)
            lineTo(right, 436f)
        }
    }

    private val arches = listOf(
        Arch(84f, 258f, 40f, "#4F8A84", "#1FB3A6"),
        Arch(146f, 290f, 36f, "#4A7F79", "#18A196"),
        Arch(204f, 318f, 32f, "#45746F", "#138D85"),
    )
    private val animation = ValueAnimator.ofFloat(0f, 3f).apply {
        duration = 1050L
        repeatCount = ValueAnimator.INFINITE
        interpolator = LinearInterpolator()
        addUpdateListener { animator ->
            val frame = (animator.animatedValue as Float).toInt().coerceAtMost(2)
            if (highlighted != frame) {
                highlighted = frame
                invalidate()
            }
        }
    }

    init {
        isFocusable = false
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_NO
    }

    fun show(next: State) {
        if (state == next) return
        state = next
        syncAnimation()
        invalidate()
    }

    private fun syncAnimation() {
        if (state == State.CONNECTING && isAttachedToWindow && isShown && windowVisibility == VISIBLE &&
            ValueAnimator.areAnimatorsEnabled()) {
            if (!animation.isStarted) animation.start()
        } else {
            animation.cancel()
            highlighted = 0
        }
    }

    override fun onVisibilityAggregated(isVisible: Boolean) {
        super.onVisibilityAggregated(isVisible)
        syncAnimation()
    }

    override fun onDetachedFromWindow() {
        animation.cancel()
        super.onDetachedFromWindow()
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val size = minOf(width, height).toFloat()
        val saved = canvas.save()
        canvas.translate((width - size) / 2, (height - size) / 2)
        canvas.scale(size / 408f, size / 408f)
        canvas.translate(-52f, -52f)

        if (state == State.ON) canvas.drawCircle(256f, 386f, 86f, glow)
        arches.forEachIndexed { index, arch ->
            stroke.strokeWidth = arch.width
            stroke.color = when {
                state == State.ON -> arch.onColor
                state == State.CONNECTING && index == arches.lastIndex - highlighted -> 0xff3ee6d4.toInt()
                else -> arch.offColor
            }
            canvas.drawPath(arch.path, stroke)
        }
        dot.color = when (state) {
            State.ON -> 0xffffc84f.toInt()
            State.CONNECTING -> 0xff2a5a52.toInt()
            State.OFF -> 0xff1d4a43.toInt()
        }
        canvas.drawCircle(256f, 388f, 31f, dot)
        canvas.restoreToCount(saved)
    }
}
