<script setup>
import DefaultTheme from 'vitepress/theme'
import NestBrunoLogo from './NestBrunoLogo.vue'

const { Layout } = DefaultTheme
</script>

<template>
  <Layout>
    <template #home-hero-image>
      <NestBrunoLogo />
    </template>
  </Layout>
</template>
