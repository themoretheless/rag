<script lang="ts">let { name, size = 18 }: { name: string; size?: number } = $props()</script>
<svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
  {#if name === 'layers'}<path d="m12 3 9 5-9 5-9-5 9-5Z"/><path d="m3 12 9 5 9-5M3 16l9 5 9-5"/>
  {:else if name === 'console'}<rect x="3" y="3" width="7.5" height="7.5" rx="2"/><rect x="13.5" y="3" width="7.5" height="7.5" rx="2"/><rect x="3" y="13.5" width="7.5" height="7.5" rx="2"/><rect x="13.5" y="13.5" width="7.5" height="7.5" rx="2"/>
  {:else if name === 'corpus'}<ellipse cx="12" cy="6" rx="8" ry="3"/><path d="M4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3"/>
  {:else if name === 'search'}<circle cx="11" cy="11" r="6.5"/><path d="m16 16 4.5 4.5"/>
  {:else if name === 'graph'}<circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="18" r="2.5"/><circle cx="12" cy="6" r="2.5"/><path d="m7.4 15.8 3.2-7.4m6 7.4-3.2-7.4M8.5 18h7"/>
  {:else if name === 'wiki'}<path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H20v15H6.5A2.5 2.5 0 0 0 4 20.5ZM4 18a2.5 2.5 0 0 1 2.5-2.5H20"/>
  {:else if name === 'agents'}<rect x="5" y="8" width="14" height="11" rx="3"/><path d="M12 8V4m-3 9.5h.01m5.99 0h.01M2 13h3m14 0h3"/>
  {:else if name === 'sync'}<ellipse cx="12" cy="5" rx="7" ry="2.5"/><path d="M5 5v5c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5V5M5 10v5c0 1.4 3.1 2.5 7 2.5 1.5 0 2.9-.2 4-.5"/><path d="m17 15 3 3-3 3m3-3h-7"/>
  {:else if name === 'evaluation'}<path d="M4 20v-7m6 7V5m6 15V10M2 20h20"/>
  {:else}<path d="M4 7h9m4 0h3M4 17h5m4 0h7"/><circle cx="15" cy="7" r="2"/><circle cx="11" cy="17" r="2"/>{/if}
</svg>
<style>svg{fill:none;stroke:currentColor;stroke-width:1.7;stroke-linecap:round;stroke-linejoin:round}</style>
