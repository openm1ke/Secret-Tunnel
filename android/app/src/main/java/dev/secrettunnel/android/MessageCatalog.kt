package dev.secrettunnel.android

/** The same source-message contract as the React and Swift clients. */
internal class MessageCatalog(private val messages: Map<String, String>) {
    private companion object { val placeholder = Regex("\\{(\\d+)\\}") }
    private data class Pattern(val expression: Regex, val indices: List<Int>, val replacement: String)
    private val patterns by lazy {
        messages.filterKeys { placeholder.containsMatchIn(it) }
            .toList().sortedByDescending { (key, _) -> placeholder.replace(key, "").length }
            .map { (key, value) ->
                val indices = mutableListOf<Int>()
                var offset = 0
                val expression = buildString {
                    append("^")
                    placeholder.findAll(key).forEach { match ->
                        append(Regex.escape(key.substring(offset, match.range.first)))
                        append("([\\s\\S]*?)")
                        indices.add(match.groupValues[1].toInt())
                        offset = match.range.last + 1
                    }
                    append(Regex.escape(key.substring(offset))); append("$")
                }
                Pattern(Regex(expression), indices, value)
            }
    }

    fun text(source: String, english: Boolean, args: Array<out Any?> = emptyArray()): String {
        val result = if (english) messages[source] ?: source else source
        if (args.isEmpty() && english && result == source && source.length <= 4096 && source.any { it in 'А'..'я' || it == 'ё' || it == 'Ё' }) {
            for ((expression, indices, replacement) in patterns) {
                val match = expression.matchEntire(source) ?: continue
                val values = indices.zip(match.groupValues.drop(1)).toMap()
                return placeholder.replace(replacement) { values[it.groupValues[1].toInt()] ?: it.value }
            }
        }
        return placeholder.replace(result) {
            val index = it.groupValues[1].toInt()
            if (index < args.size) args[index].toString() else it.value
        }
    }
}
