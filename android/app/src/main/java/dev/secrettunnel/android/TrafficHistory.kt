package dev.secrettunnel.android

enum class ChartRange(val label: String, val milliseconds: Long?) {
    SECOND("1 с", 1000),
    FIVE_SECONDS("5 с", 5000),
    THIRTY_SECONDS("30 с", 30000),
    MINUTE("1 мин", 60000),
    SESSION("Весь сеанс", null),
}

data class TrafficPoint(val timeMillis: Long, val rate: TrafficMeter.Rate)
data class TrafficWindow(val start: Long, val end: Long, val points: List<TrafficPoint>)

class TrafficHistory {
    private data class Bucket(val start: Long, val down: TrafficPoint, val up: TrafficPoint) {
        fun merge(other: Bucket) = Bucket(
            start,
            if (down.rate.down >= other.down.rate.down) down else other.down,
            if (up.rate.up >= other.up.rate.up) up else other.up,
        )
    }

    private var meter = TrafficMeter()
    private val live = ArrayDeque<TrafficPoint>()
    private val archive = mutableListOf<Bucket>()
    private var first: TrafficPoint? = null
    private var bucketMillis = 60000L

    @Synchronized fun clear() {
        meter = TrafficMeter()
        live.clear()
        archive.clear()
        first = null
        bucketMillis = 60000
    }

    @Synchronized fun sample(now: Long, rx: Long, tx: Long) {
        val point = TrafficPoint(now, meter.sample(now, rx, tx))
        val start = first ?: point.also { first = it }
        live.addLast(point)
        while (live.size > 300 || now - live.first().timeMillis > 60000) live.removeFirst()

        val bucketStart = start.timeMillis + (now - start.timeMillis) / bucketMillis * bucketMillis
        val bucket = Bucket(bucketStart, point, point)
        if (archive.lastOrNull()?.start == bucketStart) {
            archive[archive.lastIndex] = archive.last().merge(bucket)
        } else {
            archive.add(bucket)
        }
        if (archive.size > 720) {
            val merged = archive.chunked(2).map { group -> group.getOrNull(1)?.let(group[0]::merge) ?: group[0] }
            archive.clear()
            archive.addAll(merged)
            bucketMillis *= 2
        }
    }

    @Synchronized fun snapshot(range: ChartRange): TrafficWindow {
        val start = first ?: return TrafficWindow(0, 0, emptyList())
        val end = live.last()
        val duration = range.milliseconds
        if (duration != null) {
            val left = maxOf(start.timeMillis, end.timeMillis - duration)
            return TrafficWindow(left, end.timeMillis, live.filter { it.timeMillis >= left })
        }
        val points = (listOf(start) + archive.flatMap { listOf(it.down, it.up) } + end)
            .distinctBy { it.timeMillis }.sortedBy { it.timeMillis }
        return TrafficWindow(start.timeMillis, end.timeMillis, points)
    }
}
