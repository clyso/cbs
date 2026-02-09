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
  import LoginForm from '@/components/auth/LoginForm.vue';
</script>

<template>
  <div class="login-page">
    <LoginForm />
  </div>
</template>

<style lang="scss" scoped>
  @use '@/styles/utils' as utils;

  .login-page {
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
  }
</style>
