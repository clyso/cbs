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
  import { CIcon } from '@clyso/clyso-ui-kit';
  import { computed } from 'vue';
  import { RouteName } from '@/utils/types/router';
  import { IconName } from '@/utils/types/icon';
  import { useColorSchemeStore } from '@/stores/colorSchemeStore';

  const colorSchemeStore = useColorSchemeStore();
  const logoIcon = computed(() => {
    return colorSchemeStore.isDark
      ? IconName.BASE_LOGO_CES_LIGHT
      : IconName.BASE_LOGO_CES_DARK;
  });
</script>

<template>
  <div class="dashboard-logo">
    <RouterLink
      class="dashboard-logo__cbs"
      :to="{ name: RouteName.HOME }"
    >
      <CIcon
        :is-inline="true"
        class="dashboard-logo__cbs-icon"
        :name="logoIcon"
      />
      <span class="dashboard-logo__cbs-text">CBS</span>
    </RouterLink>

    <a
      href="https://clyso.com"
      rel="noopener"
      target="_blank"
      class="dashboard-logo__clyso"
    >
      <CIcon
        class="dashboard-logo__clyso-icon"
        :is-inline="true"
        :name="IconName.BASE_LOGO_CLYSO"
      />
    </a>
  </div>
</template>

<style lang="scss" scoped>
  @use '@/styles/utils' as utils;
  @use 'sass:math';

  $cbs-icon-size: 36px;
  $cbs-logo-gap: utils.unit(6);

  .dashboard-logo {
    display: flex;
    align-items: center;
    gap: $cbs-logo-gap;

    &__cbs {
      display: flex;
      align-items: center;
      gap: utils.unit(3);
    }

    &__cbs-icon {
      width: 40px;
      height: 40px;
      color: var(--primary-color);
    }

    &__clyso {
      position: relative;

      &::before {
        @include utils.absolute-y-center;
        left: - math.div($cbs-logo-gap, 2);
        content: '';

        height: 24px;
        width: 1px;
        background: var(--text-color-base);
        opacity: 0.3;
        pointer-events: none;
      }
    }

    &__cbs-text {
      @include utils.apply-styles(utils.$text-h1);
      line-height: 1;
      gap: utils.unit(2);
      margin-bottom: -4px;
    }

    &__clyso-icon {
      height: 16px;
      width: auto;
      fill: var(--text-color-3);
      margin-bottom: -2px;
    }
  }
</style>
