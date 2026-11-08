package dev.secrettunnel.android

import android.content.Context
import android.graphics.*
import android.graphics.drawable.GradientDrawable
import android.view.View
import kotlin.math.max
import java.util.Locale

object Palette {
    val background=Color.rgb(8,20,23)
    val surface=Color.rgb(16,35,39)
    val raised=Color.rgb(25,49,53)
    val border=Color.rgb(42,68,73)
    val text=Color.rgb(234,246,242)
    val muted=Color.rgb(157,181,182)
    val accent=Color.rgb(121,228,212)
    val upload=Color.rgb(173,183,255)
    val warning=Color.rgb(255,205,126)
    val error=Color.rgb(255,157,157)
    fun panel(color:Int=surface,radius:Float=20f,stroke:Int=0)=GradientDrawable().apply {
        setColor(color);cornerRadius=radius
        if(stroke!=0) setStroke(1,stroke)
    }
}

class TrafficMeter {
    private var previousTime=0L
    private var previousRx=0L
    private var previousTx=0L
    data class Rate(val down:Double,val up:Double)
    fun sample(now:Long,rx:Long,tx:Long):Rate {
        val elapsed=now-previousTime
        val valid=previousTime!=0L&&elapsed in 100..5000&&rx>=previousRx&&tx>=previousTx
        val rate=if(valid) Rate((rx-previousRx)*1000.0/elapsed,(tx-previousTx)*1000.0/elapsed) else Rate(0.0,0.0)
        previousTime=now;previousRx=rx;previousTx=tx
        return rate
    }
    companion object {
        fun bytes(value:Double)=when {
            value>=1048576->String.format(Locale.US,"%.1f MiB",value/1048576)
            value>=1024->String.format(Locale.US,"%.1f KiB",value/1024)
            else->"${value.toLong().coerceAtLeast(0)} B"
        }
    }
}

data class TrafficPoint(val timeMillis:Long,val rate:TrafficMeter.Rate)
class TrafficHistory {
    private var meter=TrafficMeter()
    private val points=ArrayDeque<TrafficPoint>()
    @Synchronized fun clear(){meter=TrafficMeter();points.clear()}
    @Synchronized fun sample(now:Long,rx:Long,tx:Long){points.addLast(TrafficPoint(now,meter.sample(now,rx,tx)));trim(now)}
    private fun trim(now:Long){while(points.isNotEmpty()&&(now-points.first().timeMillis>60000||points.size>60))points.removeFirst()}
    @Synchronized fun snapshot(now:Long):List<TrafficPoint>{trim(now);return points.toList()}
}

class NetworkChart(context:Context):View(context) {
    private var points=listOf<TrafficPoint>()
    private val paint=Paint(Paint.ANTI_ALIAS_FLAG)
    fun show(history:List<TrafficPoint>){points=history;invalidate()}
    override fun onDraw(canvas:Canvas) {
        super.onDraw(canvas)
        val density=resources.displayMetrics.density
        val left=4*density;val top=24*density;val bottom=height-22*density
        val peak=points.maxOfOrNull{max(it.rate.down,it.rate.up)}?.coerceAtLeast(1024.0)?:1024.0
        paint.style=Paint.Style.FILL;paint.textSize=10*density;paint.color=Palette.muted
        canvas.drawText(TrafficMeter.bytes(peak)+"/с",left,13*density,paint)
        paint.color=Palette.border;paint.strokeWidth=density
        for(i in 0..2){val y=top+(bottom-top)*i/2;canvas.drawLine(left,y,width.toFloat(),y,paint)}
        fun line(down:Boolean,color:Int) {
            if(points.size<2)return
            val path=Path()
            val now=android.os.SystemClock.elapsedRealtime()
            points.forEachIndexed{i,p->
                val x=left+((p.timeMillis-(now-60000))/60000f).coerceIn(0f,1f)*(width-left)
                val y=(bottom-(if(down)p.rate.down else p.rate.up)/peak*(bottom-top)).toFloat()
                if(i==0)path.moveTo(x,y)else path.lineTo(x,y)
            }
            paint.color=color;paint.style=Paint.Style.STROKE;paint.strokeWidth=2*density
            paint.strokeJoin=Paint.Join.ROUND;canvas.drawPath(path,paint)
        }
        line(true,Palette.accent);line(false,Palette.upload)
        paint.style=Paint.Style.FILL;paint.textSize=10*density;paint.color=Palette.muted
        canvas.drawText("−60 с",left,height-3*density,paint)
        canvas.drawText("Сейчас",width-paint.measureText("Сейчас"),height-3*density,paint)
    }
}

class NavigationIcon(context:Context,private val kind:String,var tint:Int):View(context) {
    private val paint=Paint(Paint.ANTI_ALIAS_FLAG)
    override fun onDraw(canvas:Canvas) {
        super.onDraw(canvas);canvas.save();canvas.translate((width-height)/2f,0f);canvas.scale(height/24f,height/24f)
        paint.color=tint;paint.style=Paint.Style.STROKE;paint.strokeWidth=1.8f;paint.strokeCap=Paint.Cap.ROUND;paint.strokeJoin=Paint.Join.ROUND
        fun line(x:Float,y:Float,x2:Float,y2:Float)=canvas.drawLine(x,y,x2,y2,paint)
        when(kind) {
            "home"->{val p=Path();p.moveTo(12f,3f);p.lineTo(20f,6f);p.lineTo(19f,14f);p.quadTo(17f,19f,12f,22f);p.quadTo(7f,19f,5f,14f);p.lineTo(4f,6f);p.close();canvas.drawPath(p,paint);line(8f,12f,11f,15f);line(11f,15f,16f,9f)}
            "servers"->{for(y in listOf(4f,13f)){canvas.drawRoundRect(4f,y,20f,y+7f,2f,2f,paint);line(8f,y+3.5f,8.1f,y+3.5f);line(13f,y+3.5f,17f,y+3.5f)}}
            "proxies"->{line(4f,8f,20f,8f);line(16f,4f,20f,8f);line(20f,8f,16f,12f);line(20f,17f,4f,17f);line(4f,17f,8f,13f);line(4f,17f,8f,21f)}
            "apps"->{for(x in listOf(4f,14f))for(y in listOf(4f,14f))canvas.drawRoundRect(x,y,x+6f,y+6f,1.5f,1.5f,paint)}
            "logs"->{canvas.drawRoundRect(4f,3f,20f,21f,2f,2f,paint);for(y in listOf(8f,12f,16f))line(8f,y,16f,y)}
        }
        canvas.restore()
    }
}
