const ICON_ATTRS = 'class="pt-icon" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"';

const GRID_ICON = `<svg ${ICON_ATTRS}><path d="M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z"/></svg>`;
// Converted from the official RGB AI artwork: https://xueqiu.com/about/logo
// Original paths/transforms preserved; conversion does not grant brand authorization.
const XUEQIU_ICON = `<svg class="pt-icon xueqiu-icon" viewBox="0 0 24 24" fill="var(--platform-icon-fill, #287DFF)" aria-hidden="true" focusable="false">
  <g transform="matrix(0.1348314607,0,0,0.1348314607,-35.6576089888,-60.7921146067)">
    <path transform="matrix(1,0,0,-1,392.0107,594.125)" d="M0 0C-1.881 0-3.756 .723-5.178 2.145L-27.359 24.325C-37.738 34.704-37.738 51.591-27.359 61.969L-11.797 77.532C-8.938 80.39-4.304 80.39-1.445 77.532 1.414 74.673 1.414 70.038-1.445 67.179L-17.007 51.617C-21.678 46.947-21.678 39.348-17.007 34.678L.187 17.484C9.725 27.532 14.981 40.83 14.804 54.871 14.633 68.479 9.274 81.408-.287 91.276-9.848 101.146-22.596 106.904-36.182 107.489-41.939 107.734-47.625 107.08-53.087 105.537-56.979 104.439-61.022 106.7-62.121 110.591-63.22 114.482-60.957 118.526-57.066 119.625-50.103 121.592-42.864 122.43-35.552 122.115-18.208 121.368-1.95 114.034 10.229 101.463 22.401 88.898 29.225 72.417 29.443 55.055 29.704 34.346 20.687 14.904 4.708 1.716 4.699 1.709 4.641 1.661 4.633 1.655 3.279 .547 1.637 0 0 0" />
    <path transform="matrix(1,0,0,-1,353.4595,608.0547)" d="M0 0C-.998 0-1.997 .021-2.998 .064-20.341 .811-36.6 8.146-48.778 20.716-60.951 33.281-67.775 49.763-67.993 67.125-68.253 87.834-59.238 107.276-43.257 120.464L-43.206 120.506C-40.295 122.907-36.039 122.703-33.372 120.035L-11.19 97.854C-6.163 92.827-3.394 86.142-3.394 79.032-3.394 71.922-6.163 65.237-11.19 60.21L-26.753 44.647C-29.611 41.79-34.246 41.789-37.105 44.647-39.964 47.507-39.964 52.142-37.105 55L-21.543 70.563C-19.28 72.825-18.034 75.833-18.034 79.032-18.034 82.231-19.281 85.24-21.543 87.502L-38.734 104.694C-48.278 94.636-53.53 81.343-53.354 67.309-53.183 53.701-47.823 40.771-38.263 30.903-28.702 21.033-15.954 15.276-2.368 14.691 3.386 14.443 9.075 15.101 14.537 16.643 18.427 17.741 22.472 15.479 23.571 11.588 24.67 7.697 22.407 3.653 18.517 2.554 12.507 .856 6.291 0 0 0" />
  </g>
</svg>`;
const COMBINATION_ICON = `<svg viewBox="0 0 24 24" class="pt-icon" aria-hidden="true" focusable="false" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4z"/><path d="m3.3 7 8.7 5 8.7-5M12 22V12"/></svg>`;
const WEIBO_ICON = `<svg ${ICON_ATTRS}><path d="M10.1 20.3c-4 .4-7.4-1.4-7.7-4-.3-2.6 2.8-5 6.7-5.4 4-.4 7.4 1.4 7.7 4 .3 2.6-2.8 5-6.7 5.4zM19 12.8c-.3-.1-.6-.2-.4-.6.4-1 .4-1.8 0-2.4-.8-1.1-2.9-1.1-5.4 0 0 0-.8.3-.6-.3.4-1.2.3-2.2-.3-2.8-1.3-1.3-4.9 0-7.9 3.1C1.3 10.9 0 13.3 0 15.3c0 4 5.1 6.4 10.1 6.4 6.5 0 10.9-3.8 10.9-6.8 0-1.8-1.6-2.9-2.9-3.3z"/></svg>`;
const TWITTER_ICON = `<svg ${ICON_ATTRS}><path d="M14.2 10.2 23 0h-2.1l-7.6 8.8L7.3 0H.3l9.2 13.3L.3 24h2.1l8-9.3 6.4 9.3h7l-9.6-13.8zm-2.8 3.3-.9-1.3L3.1 1.6h3.2l6 8.5.9 1.3 7.8 11.1h-3.2l-6.4-9z"/></svg>`;
const ZSXQ_ICON = `<svg ${ICON_ATTRS} viewBox="0 0 26 26" fill-rule="evenodd"><path d="M13 0a1.6 1.6 0 1 1 0 3.2A9.8 9.8 0 1 0 22.8 13a1.6 1.6 0 1 1 3.2 0A13 13 0 1 1 13 0zm8 2a3 3 0 1 1 0 6 3 3 0 0 1 0-6z"/></svg>`;
const TRUTH_ICON = `<svg class="pt-icon" viewBox="0 0 24 24" fill="none" aria-hidden="true"><rect x="2.5" y="3.5" width="4.8" height="4.2" fill="#2DD0B3"/><path d="M9.2 3.5H21.5V7.7H14.5V20.5H9.2V3.5z" fill="currentColor"/></svg>`;

export const PLATFORM_BADGES = {
  "": { label: "全部", shortLabel: "全部", icon: GRID_ICON },
  xueqiu: { label: "雪球", shortLabel: "雪球", icon: XUEQIU_ICON },
  combination: { label: "雪球组合", shortLabel: "组合", icon: COMBINATION_ICON },
  weibo: { label: "微博", shortLabel: "微博", icon: WEIBO_ICON },
  twitter: { label: "X", shortLabel: "X", icon: TWITTER_ICON },
  zsxq: { label: "知识星球", shortLabel: "星球", icon: ZSXQ_ICON },
  truth: { label: "Truth Social", shortLabel: "Truth", icon: TRUTH_ICON },
};

export const PLATFORM_LABELS = Object.fromEntries(
  Object.entries(PLATFORM_BADGES).filter(([key]) => key).map(([key, badge]) => [key, badge.label])
);
export const PLATFORM_SHORT_LABELS = Object.fromEntries(
  Object.entries(PLATFORM_BADGES).filter(([key]) => key).map(([key, badge]) => [key, badge.shortLabel])
);
export const PLATFORM_ICONS = Object.fromEntries(
  Object.entries(PLATFORM_BADGES).map(([key, badge]) => [key, badge.icon])
);
export const PLATFORM_TABS = Object.keys(PLATFORM_BADGES);
