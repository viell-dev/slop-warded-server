<script setup lang="ts">
import { ref } from "vue";

const count = ref(0);
const name = ref("you");
const answer = ref("");

async function greet() {
  const response = await fetch(`/api/greeting/${encodeURIComponent(name.value)}`);
  answer.value = response.ok ? (await response.json()).text : "The server did not answer";
}
</script>

<template>
  <main>
    <h1>It works</h1>
    <p>Edit <code>web/src/App.vue</code> for the page and <code>src/api.rs</code> for the API.</p>
    <button @click="count++">Count: {{ count }}</button>
    <form @submit.prevent="greet">
      <input v-model="name" aria-label="Name" required>
      <button>Greet</button>
      <output>{{ answer }}</output>
    </form>
  </main>
</template>
