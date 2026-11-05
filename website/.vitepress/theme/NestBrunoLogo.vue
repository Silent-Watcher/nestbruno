<script setup>
/**
 * NestBruno mark: a NestJS controller sending its routes into a Bruno
 * collection. Pure SVG + CSS; no animation dependencies. All motion is
 * disabled for users who prefer reduced motion.
 */
</script>

<template>
  <svg
    class="nb-logo"
    viewBox="0 0 240 140"
    role="img"
    aria-label="NestBruno: a NestJS controller synchronizing routes into a Bruno collection"
  >
    <defs>
      <linearGradient id="nb-amber" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0" stop-color="#fbbf24" />
        <stop offset="1" stop-color="#f59e0b" />
      </linearGradient>
      <linearGradient id="nb-slate" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#334155" />
        <stop offset="1" stop-color="#1e293b" />
      </linearGradient>
    </defs>

    <!-- controller (left) -->
    <g class="nb-controller">
      <rect x="8" y="38" width="64" height="64" rx="12" fill="url(#nb-slate)" />
      <text x="40" y="66" text-anchor="middle" class="nb-glyph">@</text>
      <path d="M30 78l-7 7 7 7M50 78l7 7-7 7" class="nb-code" />
    </g>

    <!-- flowing routes (middle) -->
    <g class="nb-routes">
      <path class="nb-flow nb-flow-1" d="M78 56 H 158" />
      <path class="nb-flow nb-flow-2" d="M78 70 H 158" />
      <path class="nb-flow nb-flow-3" d="M78 84 H 158" />
    </g>

    <!-- Bruno folder (right) -->
    <g class="nb-folder">
      <path
        d="M156 44h26l8 10h30a10 10 0 0 1 10 10v32a10 10 0 0 1-10 10h-64a10 10 0 0 1-10-10V54a10 10 0 0 1 10-10Z"
        fill="url(#nb-amber)"
      />
      <rect x="152" y="66" width="72" height="24" rx="5" fill="#fffbeb" />
      <circle cx="163" cy="78" r="4" fill="#059669" />
      <text x="172" y="82" class="nb-method">GET</text>
    </g>

    <!-- nest arcs (underneath) -->
    <g class="nb-nest">
      <path d="M96 122c16 8 56 8 72 0" />
      <path d="M104 130c14 6 44 6 56 0" />
    </g>
  </svg>
</template>

<style scoped>
.nb-logo {
  width: 100%;
  max-width: 300px;
  height: auto;
  display: block;
}
.nb-glyph {
  font: 700 26px ui-monospace, SFMono-Regular, Menlo, monospace;
  fill: #fbbf24;
}
.nb-code {
  stroke: #e2e8f0;
  stroke-width: 3.5;
  fill: none;
  stroke-linecap: round;
  stroke-linejoin: round;
}
.nb-flow {
  stroke: #f59e0b;
  stroke-width: 4;
  stroke-linecap: round;
  fill: none;
  stroke-dasharray: 10 12;
}
.nb-flow-2 {
  stroke: #fbbf24;
}
.nb-flow-3 {
  stroke: #10b981;
}
.nb-method {
  font: 700 15px ui-monospace, SFMono-Regular, Menlo, monospace;
  fill: #1e293b;
}
.nb-nest path {
  stroke: #94a3b8;
  stroke-width: 4;
  stroke-linecap: round;
  fill: none;
  opacity: 0.6;
}

@media (prefers-reduced-motion: no-preference) {
  .nb-flow {
    animation: nb-march 1.2s linear infinite;
  }
  .nb-flow-2 {
    animation-duration: 1.5s;
  }
  .nb-flow-3 {
    animation-duration: 1.8s;
  }
  .nb-controller {
    animation: nb-float 3.2s ease-in-out infinite;
  }
  .nb-folder {
    animation: nb-float 3.2s ease-in-out infinite reverse;
  }
}

@keyframes nb-march {
  to {
    stroke-dashoffset: -22;
  }
}
@keyframes nb-float {
  0%,
  100% {
    transform: translateY(0);
  }
  50% {
    transform: translateY(-3px);
  }
}
</style>
