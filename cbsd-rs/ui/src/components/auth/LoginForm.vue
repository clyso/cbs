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
  import { CTile, CIcon } from '@clyso/clyso-ui-kit';
  import GoogleLoginButton from './GoogleLoginButton.vue';
  import { useAuthStore } from '@/stores/auth';
  import { IconName } from '@/utils/types/icon';

  const { login } = useAuthStore();
</script>

<template>
  <CTile class="login-form">
    <template #header>
      <div class="login-form__header">
        <CIcon
          :name="IconName.BASE_LOGO_CES_LIGHT"
          alt="Clyso"
          class="login-form__logo"
        />
        <h1 class="login-form__title">Welcome to CBS</h1>
        <p class="login-form__subtitle">Please sign in to continue</p>
      </div>
    </template>
    <div class="login-form__content">
      <GoogleLoginButton @click="login" />
    </div>
    <template #footer>
      <div class="login-form__footer">
        &copy; {{ new Date().getFullYear() }} Clyso GmbH.
      </div>
    </template>
  </CTile>
</template>

<style lang="scss" scoped>
  @use '@/styles/utils' as utils;

  .login-form {
    width: 100%;
    max-width: 440px;
    box-shadow: var(--notification-shadow);
    justify-content: center;

    &__logo {
      width: 48px;
      height: 48px;
      margin-bottom: 24px;
    }

    &__header {
      display: flex;
      flex-direction: column;
      align-items: center;
    }

    &__title {
      margin: 0 0 8px 0;
    }

    &__subtitle {
      color: var(--text-color-3);
      margin-bottom: 16px;
    }

    &__content {
      display: flex;
      justify-content: center;
    }

    &__footer {
      width: 100%;
      text-align: center;
      font-size: 0.75rem;
      color: var(--text-color-disabled);
      margin-top: 16px;
    }
  }
</style>
