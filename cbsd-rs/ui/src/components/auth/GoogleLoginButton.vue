<!--
  - Copyright © 2026 Clyso GmbH
  -
  -  Licensed under the GNU Affero General Public License, Version 3.0 (the "License");
  -  you may not use this file except in compliance with the License.
  -  You may obtain a copy of the License at
  -
  -  https://www.gnu.org/licenses/agpl-3.0.html
  -
  -  Unless required by applicable law or agreed to in writing, software
  -  distributed under the License is distributed on an "AS IS" BASIS,
  -  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
  -  See the License for the specific language governing permissions and
  -  limitations under the License.
  -->

<script setup lang="ts">
  const emit = defineEmits(['click']);

  const handleClick = () => {
    emit('click');
  };
</script>

<template>
  <button
    class="g-button"
    type="button"
    @click="handleClick"
  >
    <div class="g-button__icon-wrapper">
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="0 0 48 48"
        class="g-button__icon"
      >
        <path
          fill="#EA4335"
          d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z"
        ></path>
        <path
          fill="#4285F4"
          d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z"
        ></path>
        <path
          fill="#FBBC05"
          d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z"
        ></path>
        <path
          fill="#34A853"
          d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z"
        ></path>
      </svg>
    </div>
    <span class="g-button__text">Sign in with Google</span>
  </button>
</template>

<style lang="scss" scoped>
  @use 'sass:color';

  $google-dark-bg: #131314;
  $google-white-overlay: #ffffff;
  $google-border-gray: #8e918f;

  .g-button {
    background-color: $google-dark-bg;
    border: 1px solid $google-border-gray;
    border-radius: 20px;

    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 10px 12px;
    gap: 10px;

    cursor: pointer;
    transition: background-color 0.218s;
    font-family: 'Roboto', arial, sans-serif;
    overflow: hidden;

    &:hover {
      background-color: color.mix($google-white-overlay, $google-dark-bg, 8%);
    }

    &:active {
      background-color: color.mix($google-white-overlay, $google-dark-bg, 12%);
    }

    &:focus {
      outline: 2px solid #8ab4f8;
      outline-offset: 2px;
    }

    &__icon-wrapper {
      display: flex;
      align-items: center;
      justify-content: center;
      width: 20px;
      height: 20px;
      flex-shrink: 0;
    }

    &__icon {
      width: 100%;
      height: 100%;
      display: block;
    }

    &__text {
      color: #e3e3e3;
      font-size: 14px;
      font-weight: 500;
      line-height: 1;
      letter-spacing: 0.25px;
      white-space: nowrap;
    }
  }
</style>
