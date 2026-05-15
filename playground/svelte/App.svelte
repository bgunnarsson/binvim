<script lang="ts">
    import { onMount } from "svelte";

    type Todo = { id: number; text: string; done: boolean };

    let todos: Todo[] = [
        { id: 1, text: "Write tests", done: false },
        { id: 2, text: "Read the docs", done: true },
    ];
    let draft = "";

    $: openCount = todos.filter((t) => !t.done).length;

    function toggle(id: number) {
        todos = todos.map((t) => (t.id === id ? { ...t, done: !t.done } : t));
    }

    function add() {
        if (!draft.trim()) return;
        todos = [...todos, { id: Date.now(), text: draft.trim(), done: false }];
        draft = "";
    }

    onMount(() => {
        console.log("mounted with", todos.length, "todos");
    });
</script>

<main>
    <h1>Todos ({openCount} open)</h1>

    <ul>
        {#each todos as todo (todo.id)}
            <li class:done={todo.done}>
                <input
                    type="checkbox"
                    checked={todo.done}
                    on:change={() => toggle(todo.id)}
                />
                <span>{todo.text}</span>
            </li>
        {/each}
    </ul>

    <form on:submit|preventDefault={add}>
        <input bind:value={draft} placeholder="New todo…" />
        <button type="submit">Add</button>
    </form>
</main>

<style>
    main {
        font-family: system-ui, sans-serif;
        max-width: 480px;
        margin: 2rem auto;
    }

    li.done span {
        text-decoration: line-through;
        opacity: 0.6;
    }
</style>
