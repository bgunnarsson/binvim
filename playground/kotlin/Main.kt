package playground

import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking

sealed interface Shape {
    val name: String
    fun area(): Double
}

data class Circle(val radius: Double) : Shape {
    override val name = "circle"
    override fun area() = Math.PI * radius * radius
}

data class Square(val side: Double) : Shape {
    override val name = "square"
    override fun area() = side * side
}

enum class Status(val label: String) {
    Active("active"),
    Pending("pending"),
    Banned("banned");

    fun isActive(): Boolean = this == Active
}

data class User(
    val id: Int,
    val name: String,
    val email: String? = null,
    val status: Status = Status.Active,
)

fun describe(shape: Shape): String = when (shape) {
    is Circle -> "circle r=${shape.radius}"
    is Square -> "square s=${shape.side}"
}

suspend fun fetchUser(id: Int): User {
    delay(10)
    return User(id = id, name = "user-$id", email = "user$id@example.com")
}

fun main() = runBlocking {
    val shapes: List<Shape> = listOf(Circle(2.0), Square(3.0), Circle(5.0))
    shapes
        .sortedByDescending { it.area() }
        .forEach { println(describe(it)) }

    val users = coroutineScope {
        (1..3).map { async { fetchUser(it) } }.awaitAll()
    }
    users
        .filter { it.status.isActive() }
        .forEach { println("${it.name} <${it.email}>") }
}
